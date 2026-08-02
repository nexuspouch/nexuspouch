use serde_json::{json, Map, Value};
use std::io::Read;

pub struct Client {
    base: String,
    token: String,
    agent: Option<String>,
}

impl Client {
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            token: token.into(),
            agent: None,
        }
    }

    pub fn with_agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    fn auth(&self, req: ureq::Request) -> ureq::Request {
        let req = req.set("Authorization", &format!("Bearer {}", self.token));
        match &self.agent {
            Some(a) => req.set("x-agent-id", a),
            None => req,
        }
    }

    pub fn resolve_url(&self, uri: &str) -> String {
        format!(
            "{}/api/v1/uri/resolve?uri={}",
            self.base,
            urlencoding(uri)
        )
    }

    pub fn read_url(&self, uri: &str) -> String {
        format!("{}/api/v1/read?uri={}", self.base, urlencoding(uri))
    }

    pub fn list_url(&self, uri: &str) -> String {
        format!("{}/api/v1/list?uri={}", self.base, urlencoding(uri))
    }

    pub fn events_url(&self) -> String {
        if self.token.is_empty() {
            format!("{}/api/v1/events", self.base)
        } else {
            format!(
                "{}/api/v1/events?token={}",
                self.base,
                urlencoding(&self.token)
            )
        }
    }

    pub fn health(&self) -> Result<Value, String> {
        let url = format!("{}/api/v1/health", self.base);
        self.get_json(&url)
    }

    pub fn resolve(&self, uri: &str) -> Result<Value, String> {
        let url = self.resolve_url(uri);
        self.get_json(&url)
    }

    pub fn read_text(&self, uri: &str) -> Result<Vec<u8>, String> {
        let url = self.read_url(uri);
        let resp = self
            .auth(ureq::get(&url))
            .call()
            .map_err(|e| e.to_string())?;
        resp.into_string()
            .map(|s| s.into_bytes())
            .map_err(|e| e.to_string())
    }

    pub fn read_bytes(&self, uri: &str, offset: i64, length: usize) -> Result<Vec<u8>, String> {
        let url = format!(
            "{}/api/v1/read?uri={}&offset={}&length={}",
            self.base,
            urlencoding(uri),
            offset,
            length
        );
        let resp = match self.auth(ureq::get(&url)).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(code, resp)) => {
                return Err(api_status_err(
                    code,
                    resp.into_string().unwrap_or_default(),
                ));
            }
            Err(e) => return Err(e.to_string()),
        };
        let mut buf = Vec::new();
        resp.into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| e.to_string())?;
        Ok(buf)
    }

    pub fn list(&self, uri: &str) -> Result<Value, String> {
        let url = self.list_url(uri);
        self.get_json(&url)
    }

    pub fn recent_events(&self, limit: usize) -> Result<Value, String> {
        let url = format!("{}/api/v1/events/recent?limit={}", self.base, limit);
        self.get_json(&url)
    }

    pub fn search(
        &self,
        q: &str,
        space: Option<&str>,
        device: Option<&str>,
        state: Option<&str>,
        limit: usize,
    ) -> Result<Value, String> {
        self.search_ex(q, space, device, state, limit, false, None, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn search_ex(
        &self,
        q: &str,
        space: Option<&str>,
        device: Option<&str>,
        state: Option<&str>,
        limit: usize,
        semantic: bool,
        filter: Option<&crate::store::SearchFilter>,
        opts: Option<&crate::store::SearchOptions>,
    ) -> Result<Value, String> {
        let mut url = format!("{}/api/v1/search?q={}", self.base, urlencoding(q));
        if let Some(s) = space {
            url.push_str(&format!("&space={}", urlencoding(s)));
        }
        if let Some(d) = device {
            url.push_str(&format!("&device={}", urlencoding(d)));
        }
        if let Some(st) = state {
            url.push_str(&format!("&state={}", urlencoding(st)));
        }
        url.push_str(&format!("&limit={}", limit.max(1)));
        if semantic {
            url.push_str("&semantic=true");
        }
        if let Some(f) = filter {
            if let Some(a) = f.agent.as_deref() {
                url.push_str(&format!("&agent={}", urlencoding(a)));
            }
            if let Some(p) = f.project.as_deref() {
                url.push_str(&format!("&project={}", urlencoding(p)));
            }
            if let Some(s) = f.since_ms {
                url.push_str(&format!("&since_ms={s}"));
            }
            if let Some(u) = f.until_ms {
                url.push_str(&format!("&until_ms={u}"));
            }
        }
        if let Some(o) = opts {
            if let Some(r) = o.rerank {
                url.push_str(&format!("&rerank={r}"));
            }
            if let Some(d) = o.dedup {
                url.push_str(&format!("&dedup={d}"));
            }
            if let Some(c) = o.context_turns {
                url.push_str(&format!("&context_turns={c}"));
            }
        }
        self.get_json(&url)
    }

    /// R3: record recall feedback (`click` / `adopt` / `wrong`) via store op.
    pub fn recall_feedback(
        &self,
        kind: &str,
        query: &str,
        uri: &str,
        rank: Option<u32>,
        note: Option<&str>,
    ) -> Result<Value, String> {
        let mut payload = Map::new();
        payload.insert("kind".into(), json!(kind));
        payload.insert("query".into(), json!(query));
        payload.insert("uri".into(), json!(uri));
        if let Some(r) = rank {
            payload.insert("rank".into(), json!(r));
        }
        if let Some(n) = note {
            payload.insert("note".into(), json!(n));
        }
        payload.insert("source".into(), json!("sdk"));
        self.store_op("recall.feedback", payload)
    }

    /// POST /api/v1/store frame. Returns the `data` object on success, or
    /// an Err carrying the store error code + message on `{"op":"error"}`.
    pub fn store_op(&self, op: &str, payload: Map<String, Value>) -> Result<Value, String> {
        let url = format!("{}/api/v1/store", self.base);
        let body = json!({"op": op, "payload": payload});
        let resp = match self
            .auth(ureq::post(&url))
            .set("Content-Type", "application/json")
            .send_bytes(body.to_string().as_bytes())
        {
            Ok(r) => r,
            Err(ureq::Error::Status(code, resp)) => {
                return Err(api_status_err(
                    code,
                    resp.into_string().unwrap_or_default(),
                ));
            }
            Err(e) => return Err(e.to_string()),
        };
        let val = parse_json_response(resp)?;
        if val.get("op").and_then(|v| v.as_str()) == Some("error") {
            let code = val
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("error");
            let msg = val
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Err(format!("{code}: {msg}"));
        }
        Ok(val.get("data").cloned().unwrap_or(Value::Null))
    }

    fn get_json(&self, url: &str) -> Result<Value, String> {
        match self.auth(ureq::get(url)).call() {
            Ok(r) => parse_json_response(r),
            Err(ureq::Error::Status(code, resp)) => Err(api_status_err(
                code,
                resp.into_string().unwrap_or_default(),
            )),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn parse_json_response(resp: ureq::Response) -> Result<Value, String> {
    let status = resp.status();
    let body = resp.into_string().unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(api_status_err(status, body));
    }
    serde_json::from_str(&body).map_err(|e| format!("bad json: {e}"))
}

fn api_status_err(code: u16, body: String) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(&body) {
        let code = v.get("error").and_then(|c| c.as_str()).unwrap_or("http_error");
        let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or(&body);
        return format!("{code}: {msg}");
    }
    format!("http_{code}: {body}")
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_builders() {
        let c = Client::new("http://127.0.0.1:8787", "secret");
        let uri = "store://artifacts/aaaaaaaaaaaaaaaa/a/b.txt";
        assert!(c.resolve_url(uri).contains("/api/v1/uri/resolve?uri="));
        assert!(c.read_url(uri).contains("/api/v1/read?uri="));
        assert!(c.events_url().contains("token=secret"));
    }
}
