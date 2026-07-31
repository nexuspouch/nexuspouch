use axum::{
    extract::{ConnectInfo, State, WebSocketUpgrade},
    http::HeaderMap,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use clap::Parser;
use nexuspouch::{
    admin::{self, handler::AdminState, auth::is_loopback},
    noise::Identity,
    peer::{advertise_local_ws, PairingHub, PeerServer, PeerStore, SessionRegistry},
    protocol,
    store::{gc, Local},
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "nexuspouch")]
struct Args {
    #[arg(long, default_value = "./data")]
    root: PathBuf,

    #[arg(long, default_value = ":8787")]
    listen: String,

    #[arg(long, default_value = "nexuspouch")]
    name: String,

    #[arg(long, env = "NEXUSPOUCH_ADMIN_TOKEN")]
    admin_token: Option<String>,

    #[arg(long, env = "SHEPAW_ADMIN_TOKEN")]
    shepaw_admin_token: Option<String>,

    #[arg(long, env = "NEXUSPOUCH_CHANNEL_ENDPOINT")]
    channel: Option<String>,

    #[arg(long, env = "SHEPAW_CHANNEL_ENDPOINT")]
    shepaw_channel: Option<String>,

    #[arg(long)]
    device: Option<String>,
}

#[derive(Deserialize)]
struct StoreBody {
    op: String,
    #[serde(default)]
    payload: Map<String, Value>,
}

fn parse_listen(listen: &str) -> String {
    if listen.starts_with(':') {
        format!("0.0.0.0{listen}")
    } else {
        listen.to_string()
    }
}

fn admin_token(args: &Args) -> String {
    args.admin_token
        .clone()
        .or_else(|| args.shepaw_admin_token.clone())
        .unwrap_or_default()
}

fn channel_endpoint(args: &Args) -> String {
    args.channel
        .clone()
        .or_else(|| args.shepaw_channel.clone())
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let listen = parse_listen(&args.listen);
    let token = admin_token(&args);
    let channel_endpoint = channel_endpoint(&args);

    let id_path = args.root.join(".system").join("noise_identity.json");
    let identity = Arc::new(Identity::load_or_create(&id_path)?);
    let mut device = identity.fingerprint();
    if let Some(ref want) = args.device {
        if want != &device {
            eprintln!("-device {want} does not match Noise fingerprint {device}");
            std::process::exit(1);
        }
        device = want.clone();
    }

    let store = Arc::new(Local::open(&args.root, &device)?);
    if let Ok(n) = gc::gc_staging(&store, Duration::ZERO) {
        if n > 0 {
            tracing::info!("gc staging: removed {n} abandoned uploads");
        }
    }
    if let Ok(b) = gc::gc_recycle(&store, Duration::ZERO) {
        if b > 0 {
            tracing::info!("gc recycle: purged {b} bytes");
        }
    }
    store.start_periodic_gc(Duration::from_secs(3600));

    let peers = Arc::new(PeerStore::new(&args.root));
    let hub = Arc::new(PairingHub::new(
        Arc::clone(&identity),
        Arc::clone(&peers),
        &args.name,
    ));
    let sessions = Arc::new(SessionRegistry::new());
    let local_endpoint = advertise_local_ws(&args.listen);

    let peer_srv = Arc::new(PeerServer {
        store: Arc::clone(&store),
        hub: Arc::clone(&hub),
        peers: Arc::clone(&peers),
        sessions: Arc::clone(&sessions),
        identity: Arc::clone(&identity),
        device_name: args.name.clone(),
        local_endpoint: local_endpoint.clone(),
        channel_endpoint: channel_endpoint.clone(),
    });

    let admin_state = Arc::new(AdminState {
        store: Arc::clone(&store),
        auth: nexuspouch::AuthConfig {
            token: token.clone(),
        },
        device: device.clone(),
        hub: Some(Arc::clone(&hub)),
        peers: Some(Arc::clone(&peers)),
        sessions: Some(Arc::clone(&sessions)),
        identity: Some(Arc::clone(&identity)),
        listen: args.listen.clone(),
        channel_endpoint: channel_endpoint.clone(),
    });

    let device_log = device.clone();
    let admin_state_health = Arc::clone(&admin_state);
    let admin_state_store = Arc::clone(&admin_state);
    let peer_srv_ws = Arc::clone(&peer_srv);

    let app = Router::new()
        .route(
            "/health",
            get(move || {
                let device = device.clone();
                async move {
                    Json(json!({
                        "ok": true,
                        "device": device,
                        "protocol": protocol::PROTOCOL_VERSION,
                        "noise": true,
                        "fingerprint": device,
                    }))
                }
            }),
        )
        .route(
            "/store",
            post(
                move |ConnectInfo(addr): ConnectInfo<SocketAddr>,
                      headers: HeaderMap,
                      Json(body): Json<StoreBody>| {
                    let state = Arc::clone(&admin_state_store);
                    async move {
                        let loopback = is_loopback(&addr.ip().to_string());
                        let payload = body.payload;
                        match admin::store_handler(
                            state,
                            headers,
                            loopback,
                            body.op,
                            payload,
                        ) {
                            Ok(data) => Json(json!({"op": "result", "data": data})).into_response(),
                            Err(e) => Json(json!({
                                "op": "error",
                                "code": e.code,
                                "message": e.msg,
                            }))
                            .into_response(),
                        }
                    }
                },
            ),
        )
        .route(
            "/peer/ws",
            get(
                move |ws: WebSocketUpgrade, State(ps): State<Arc<PeerServer>>| async move {
                    ws.on_upgrade(move |socket| {
                        let ps = Arc::clone(&ps);
                        async move { ps.handle_socket(socket).await }
                    })
                },
            )
            .with_state(peer_srv_ws),
        )
        .merge(admin::router(admin_state_health));

  if token.is_empty() {
        tracing::info!("admin: no token set — rely on loopback/network policy");
    } else {
        tracing::info!("admin: token required for /admin");
    }
    if !channel_endpoint.is_empty() {
        tracing::info!("channel endpoint: {channel_endpoint}");
    }
    tracing::info!(
        "nexuspouch device={device_log} name={} root={} listen={listen} local={local_endpoint}",
        args.name,
        args.root.display()
    );

    let listener = tokio::net::TcpListener::bind(&listen).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
