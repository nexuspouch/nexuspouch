use axum::{
    extract::{ConnectInfo, State, WebSocketUpgrade},
    http::HeaderMap,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use clap::{Parser, Subcommand};
use nexuspouch::{
    admin::{self, auth::is_loopback, handler::AdminState},
    api::{self, ApiState},
    discovery::{self, Discovery},
    events::EventBus,
    noise::Identity,
    peer::{advertise_local_ws, Dialer, PairingHub, PeerServer, PeerStore, SessionRegistry},
    protocol,
    store::{gc, reprotect, Local},
    webdav::{self, WebDavState},
    TokenStore,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "nexuspouch", about = "Local store master for ShePaw")]
struct Args {
    #[arg(long, global = true, default_value = "./data")]
    root: PathBuf,

    #[arg(long, global = true, default_value = ":8787")]
    listen: String,

    #[arg(long, global = true, default_value = "nexuspouch")]
    name: String,

    #[arg(long, global = true, env = "NEXUSPOUCH_ADMIN_TOKEN")]
    admin_token: Option<String>,

    #[arg(long, global = true, env = "SHEPAW_ADMIN_TOKEN")]
    shepaw_admin_token: Option<String>,

    #[arg(long, global = true, env = "NEXUSPOUCH_CHANNEL_ENDPOINT")]
    channel: Option<String>,

    #[arg(long, global = true, env = "SHEPAW_CHANNEL_ENDPOINT")]
    shepaw_channel: Option<String>,

    #[arg(long, global = true)]
    device: Option<String>,

    #[arg(long, global = true)]
    no_mdns: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run connectivity diagnostics (local health / Channel DNS·TCP·WS) and exit
    Doctor,
    /// Create a mirror reprotect snapshot under backups/ and exit
    Reprotect {
        /// Master password (ShePaw-compatible encrypt). Falls back to NEXUSPOUCH_REPROTECT_PASSWORD.
        #[arg(long, env = "NEXUSPOUCH_REPROTECT_PASSWORD")]
        password: Option<String>,
        /// Plain tree copy instead of encrypt (debug)
        #[arg(long)]
        copy: bool,
    },
    /// Serve MCP (Model Context Protocol) over stdio for AI agents
    Mcp,
    /// Serve MCP bound to a specific agent identity (x-agent-id header)
    McpAgent {
        #[arg(long)]
        agent: String,
    },
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

fn load_identity_device(args: &Args) -> Result<(Arc<Identity>, String), Box<dyn std::error::Error>> {
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
    Ok((identity, device))
}

fn cmd_doctor(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let listen_port = discovery::parse_listen_port(&args.listen);
    let channel = channel_endpoint(args);
    let (_, device) = load_identity_device(args)?;

    let local = discovery::diag::check_local_health(listen_port);
    let channel_check = discovery::diag::check_channel(&channel);

    // Best-effort short browse (no advertise).
    let discovery = Discovery::start(&args.name, &device, listen_port, false);
    let lan = discovery::diag::check_lan(
        discovery.as_ref().map(|d| d.as_ref()),
        Some(&device),
    );

    let report = discovery::diag::DiagnosticsReport {
        local,
        lan,
        channel: channel_check,
        mdns_advertising: false,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    // Local may be down if the daemon is not running; only fail hard on Channel.
    if report.channel.configured && !report.channel.ok {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_reprotect(args: &Args, password: Option<String>, copy: bool) -> Result<(), Box<dyn std::error::Error>> {
    let (_, device) = load_identity_device(args)?;
    let store = Local::open(&args.root, &device)?;
    let mut opts = reprotect::opts_from_env(password);
    if copy {
        opts.mode = reprotect::ReprotectMode::Copy;
    }
    let out = reprotect::run(&store, opts)?;
    println!("{}", serde_json::to_string_pretty(&Value::Object(out))?);
    Ok(())
}

async fn cmd_mcp(args: &Args, agent: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let listen_port = discovery::parse_listen_port(&args.listen);
    let base = format!("http://127.0.0.1:{listen_port}");
    let token = admin_token(args);
    let (_, device) = load_identity_device(args)?;
    nexuspouch::mcp::run(base, token, device, agent).await
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    match &args.command {
        Some(Command::Doctor) => return cmd_doctor(&args),
        Some(Command::Reprotect { password, copy }) => {
            return cmd_reprotect(&args, password.clone(), *copy);
        }
        Some(Command::Mcp) => return cmd_mcp(&args, None).await,
        Some(Command::McpAgent { agent }) => return cmd_mcp(&args, Some(agent.clone())).await,
        None => {}
    }

    serve(args).await
}

async fn serve(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let listen = parse_listen(&args.listen);
    let listen_port = discovery::parse_listen_port(&args.listen);
    let local_http = format!("http://127.0.0.1:{listen_port}");
    let token = admin_token(&args);
    let channel_endpoint = channel_endpoint(&args);

    let (identity, device) = load_identity_device(&args)?;

    let store = Arc::new(Local::open(&args.root, &device)?);
    let event_bus = EventBus::open(&args.root);
    store.set_event_bus(Arc::clone(&event_bus));
    let token_store = Arc::new(TokenStore::open(&args.root));
    let auth_cfg = nexuspouch::AuthConfig::new(token.clone(), Some(Arc::clone(&token_store)));
    let agents = Arc::new(nexuspouch::AgentRegistry::open(&args.root));
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

    let dialer = Arc::new(Dialer::new(
        Arc::clone(&identity),
        Arc::clone(&peers),
        Arc::clone(&sessions),
        local_endpoint.clone(),
        channel_endpoint.clone(),
    ));
    let rpc: Arc<dyn nexuspouch::store::PeerRpc> = sessions.clone();
    store.set_peer_rpc(rpc);
    let ensure: Arc<dyn nexuspouch::store::PeerEnsure> = dialer.clone();
    store.set_peer_ensure(ensure);

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

    let listener = tokio::net::TcpListener::bind(&listen).await?;

    let mdns_discovery = if args.no_mdns {
        Discovery::start(&args.name, &device, listen_port, false)
    } else {
        Discovery::start(&args.name, &device, listen_port, true)
    };
    if let Some(ref d) = mdns_discovery {
        if d.is_advertising() {
            tracing::info!(
                "mdns: advertising {} on port {listen_port}",
                discovery::SERVICE_TYPE
            );
        }
    }
    let mdns_advertising = mdns_discovery
        .as_ref()
        .is_some_and(|d| d.is_advertising());

    let admin_state = Arc::new(AdminState {
        store: Arc::clone(&store),
        auth: auth_cfg.clone(),
        device: device.clone(),
        agents: Arc::clone(&agents),
        hub: Some(Arc::clone(&hub)),
        peers: Some(Arc::clone(&peers)),
        sessions: Some(Arc::clone(&sessions)),
        identity: Some(Arc::clone(&identity)),
        listen: args.listen.clone(),
        channel_endpoint: channel_endpoint.clone(),
        discovery: mdns_discovery,
        local_http,
        listen_port,
    });

    let api_state = Arc::new(ApiState {
        store: Arc::clone(&store),
        auth: auth_cfg.clone(),
        device: device.clone(),
        events: Arc::clone(&event_bus),
        mdns: mdns_advertising,
        agents: Arc::clone(&agents),
    });
    let webdav_state = Arc::new(WebDavState {
        store: Arc::clone(&store),
        auth: auth_cfg.clone(),
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
                let mdns = mdns_advertising;
                async move {
                    Json(json!({
                        "ok": true,
                        "device": device,
                        "protocol": protocol::PROTOCOL_VERSION,
                        "noise": true,
                        "fingerprint": device,
                        "mdns": mdns,
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
        .merge(admin::router(admin_state_health))
        .nest("/api/v1", api::router(api_state))
        .nest("/dav", webdav::router(webdav_state));

    if token.is_empty() {
        tracing::info!("admin: no token set — rely on loopback/network policy");
    } else {
        tracing::info!("admin: token required for /admin");
    }
    if !channel_endpoint.is_empty() {
        tracing::info!("channel endpoint: {channel_endpoint}");
    }
    tracing::info!("api: /api/v1  webdav: /dav  events: SSE");
    tracing::info!(
        "nexuspouch device={device_log} name={} root={} listen={listen} local={local_endpoint}",
        args.name,
        args.root.display()
    );

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
