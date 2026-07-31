package main

import (
	"encoding/json"
	"flag"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/zoujunrong/Nexuspouch/internal/admin"
	"github.com/zoujunrong/Nexuspouch/internal/noise"
	"github.com/zoujunrong/Nexuspouch/internal/peer"
	"github.com/zoujunrong/Nexuspouch/internal/protocol"
	"github.com/zoujunrong/Nexuspouch/internal/store"
)

func main() {
	root := flag.String("root", "./data", "store root directory")
	deviceFlag := flag.String("device", "", "override device_id (default: Noise fingerprint)")
	listen := flag.String("listen", ":8787", "HTTP listen address")
	adminToken := flag.String("admin-token", envFirst("NEXUSPOUCH_ADMIN_TOKEN", "SHEPAW_ADMIN_TOKEN"),
		"admin UI/API token (env NEXUSPOUCH_ADMIN_TOKEN or SHEPAW_ADMIN_TOKEN); empty = loopback-only")
	deviceName := flag.String("name", "nexuspouch", "device display name for pairing")
	channel := flag.String("channel", envFirst("NEXUSPOUCH_CHANNEL_ENDPOINT", "SHEPAW_CHANNEL_ENDPOINT"),
		"optional Channel WS endpoint for pairing QR/response (env NEXUSPOUCH_CHANNEL_ENDPOINT or SHEPAW_CHANNEL_ENDPOINT)")
	flag.Parse()

	idPath := filepath.Join(*root, ".system", "noise_identity.json")
	identity, err := noise.LoadOrCreate(idPath)
	if err != nil {
		log.Fatal(err)
	}
	device := identity.Fingerprint()
	if *deviceFlag != "" {
		if *deviceFlag != device {
			log.Fatalf("-device %s does not match Noise fingerprint %s", *deviceFlag, device)
		}
	}

	s, err := store.Open(*root, device)
	if err != nil {
		log.Fatal(err)
	}
	if n, err := s.GcStaging(0); err != nil {
		log.Printf("gc staging: %v", err)
	} else if n > 0 {
		log.Printf("gc staging: removed %d abandoned uploads", n)
	}
	if b, err := s.GcRecycle(0); err != nil {
		log.Printf("gc recycle: %v", err)
	} else if b > 0 {
		log.Printf("gc recycle: purged %d bytes", b)
	}
	s.StartPeriodicGC(time.Hour)
	peers := peer.NewStore(*root)
	hub := peer.NewPairingHub(identity, peers, *deviceName)
	sessions := peer.NewSessionRegistry()
	localEndpoint := peer.AdvertiseLocalWS(*listen)
	channelEndpoint := strings.TrimSpace(*channel)
	s.SetPeerRPC(sessions)
	dialer := peer.NewDialer(identity, peers, sessions, localEndpoint, channelEndpoint)
	s.SetPeerEnsure(dialer)
	peerSrv := &peer.Server{
		Store:           s,
		Hub:             hub,
		Peers:           peers,
		Sessions:        sessions,
		Identity:        identity,
		DeviceName:      *deviceName,
		LocalEndpoint:   localEndpoint,
		ChannelEndpoint: channelEndpoint,
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/health", func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{
			"ok": true, "device": device, "protocol": protocol.ProtocolVersion,
			"noise": true, "fingerprint": device,
		})
	})
	authCfg := admin.AuthConfig{Token: *adminToken}
	mux.Handle("/store", admin.RequireAuth(authCfg, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "POST only", http.StatusMethodNotAllowed)
			return
		}
		var body struct {
			Op      string         `json:"op"`
			Payload map[string]any `json:"payload"`
		}
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			http.Error(w, err.Error(), 400)
			return
		}
		if body.Payload == nil {
			body.Payload = map[string]any{}
		}
		// 安全：忽略请求体自报的 caller/trust/loopback（联调口仅本机身份）
		caller := device
		trust := protocol.TrustOwner
		loopback := admin.IsLoopback(r.RemoteAddr)
		data, err := s.Handle(protocol.Frame{Op: body.Op, Payload: body.Payload}, caller, trust, loopback)
		if err != nil {
			code := "internal"
			msg := err.Error()
			if oe, ok := err.(*store.OpError); ok {
				code = oe.Code
				msg = oe.Msg
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"op": "error", "code": code, "message": msg})
			return
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"op": "result", "data": data})
	})))
	mux.HandleFunc("/peer/ws", peerSrv.HandleWS)

	adminSrv := &admin.Server{
		Store:           s,
		Device:          device,
		Auth:            admin.AuthConfig{Token: *adminToken},
		Hub:             hub,
		Peers:           peers,
		Sessions:        sessions,
		Identity:        identity,
		Listen:          *listen,
		ChannelEndpoint: channelEndpoint,
	}
	adminSrv.Mount(mux)

	if *adminToken == "" {
		log.Printf("admin: no token set — /admin only allows loopback")
	} else {
		log.Printf("admin: token required for /admin")
	}
	if channelEndpoint != "" {
		log.Printf("channel endpoint: %s", channelEndpoint)
	}
	log.Printf("nexuspouch device=%s name=%s root=%s listen=%s local=%s (Noise IK /peer/ws)",
		device, *deviceName, *root, *listen, localEndpoint)
	if err := http.ListenAndServe(*listen, mux); err != nil {
		log.Println(err)
		os.Exit(1)
	}
}

func envFirst(keys ...string) string {
	for _, k := range keys {
		if v := os.Getenv(k); v != "" {
			return v
		}
	}
	return ""
}
