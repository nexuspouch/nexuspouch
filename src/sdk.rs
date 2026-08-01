use serde_json::Value;

pub struct Client {
    base: String,
    token: String,
}

impl Client {
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            token: token.into(),
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

    pub fn resolve(&self, uri: &str) -> Result<Value, String> {
        let url = self.resolve_url(uri);
        let resp = ureq::get(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|e| e.to_string())?;
        let body = resp.into_string().map_err(|e| e.to_string())?;
        serde_json::from_str(&body).map_err(|e| e.to_string())
    }

    pub fn read_text(&self, uri: &str) -> Result<Vec<u8>, String> {
        let url = self.read_url(uri);
        let resp = ureq::get(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|e| e.to_string())?;
        resp.into_string()
            .map(|s| s.into_bytes())
            .map_err(|e| e.to_string())
    }
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
