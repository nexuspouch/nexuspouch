# Install Nexuspouch

## Binary

```bash
cargo build --release
sudo install -m 755 target/release/nexuspouch /usr/local/bin/nexuspouch
sudo useradd --system --home /var/lib/nexuspouch --shell /usr/sbin/nologin nexuspouch || true
sudo mkdir -p /var/lib/nexuspouch
sudo chown nexuspouch:nexuspouch /var/lib/nexuspouch
```

## systemd

```bash
sudo cp deploy/nexuspouch.env.example /etc/nexuspouch.env
sudo chmod 600 /etc/nexuspouch.env
# edit NEXUSPOUCH_ADMIN_TOKEN
sudo cp deploy/nexuspouch.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now nexuspouch
sudo systemctl status nexuspouch
```

Admin UI: `http://127.0.0.1:8787/admin/`

## Docker

```bash
docker build -t nexuspouch:local .
docker run -d --name nexuspouch \
  -p 8787:8787 \
  -v nexuspouch-data:/data \
  -e NEXUSPOUCH_ADMIN_TOKEN=change-me \
  nexuspouch:local
```

For LAN mDNS discovery from a container, use `--network host` (Linux) so UDP 5353 multicast works.

## Firewall notes

- TCP `8787` — HTTP / WebDAV / peer WS
- UDP `5353` — mDNS advertise/browse (optional; disable with `--no-mdns`)
