use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigStatus {
    pub mode: String,
    pub tun_enabled: Option<bool>,
}

#[derive(Clone)]
pub struct CoreController {
    base: String,
    secret: String,
    client: Client,
}

impl CoreController {
    pub fn new(port: u16, secret: String, fallback_client: Client) -> Self {
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or(fallback_client);
        Self {
            base: format!("http://127.0.0.1:{port}"),
            secret,
            client,
        }
    }

    async fn request(&self, method: reqwest::Method, path: &str) -> Result<reqwest::Response> {
        let response = self
            .client
            .request(method, format!("{}{}", self.base, path))
            .bearer_auth(&self.secret)
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await
            .context("Mihomo controller is unreachable")?;
        Ok(response)
    }

    pub async fn connections(&self) -> Result<Value> {
        let response = self.request(reqwest::Method::GET, "/connections").await?;
        if !response.status().is_success() {
            bail!("Mihomo controller rejected the connections request");
        }
        response
            .json()
            .await
            .context("Mihomo connections response was invalid")
    }

    pub async fn reload(&self, config_path: &str) -> Result<()> {
        let response = self
            .client
            .put(format!("{}/configs?force=true", self.base))
            .bearer_auth(&self.secret)
            .json(&json!({ "path": config_path }))
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .context("Mihomo controller is unreachable")?;
        if !response.status().is_success() {
            bail!("Mihomo rejected the staged configuration");
        }
        let value = response
            .json::<Value>()
            .await
            .unwrap_or_else(|_| json!({ "status": "ok" }));
        if value
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| {
                message.to_ascii_lowercase().contains("error")
                    || message.to_ascii_lowercase().contains("fail")
            })
        {
            bail!("Mihomo rejected the staged configuration");
        }
        Ok(())
    }

    pub async fn close_connections(&self) -> Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, "/connections")
            .await?;
        if !response.status().is_success() {
            bail!("Mihomo could not close existing connections");
        }
        Ok(())
    }

    pub async fn test_node_delay(&self, core_name: &str) -> Result<u64> {
        if core_name.len() > 64
            || !core_name.starts_with("CF-")
            || !core_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            bail!("The selected node cannot be tested.");
        }
        let path = format!(
            "/proxies/{core_name}/delay?url=https%3A%2F%2Fwww.gstatic.com%2Fgenerate_204&timeout=5000&expected=204"
        );
        let response = self
            .client
            .get(format!("{}{path}", self.base))
            .bearer_auth(&self.secret)
            .timeout(std::time::Duration::from_millis(6500))
            .send()
            .await
            .context("The node test did not complete")?;
        if !response.status().is_success() {
            bail!("The node test did not receive a successful response.");
        }
        let value = response
            .json::<Value>()
            .await
            .context("The node test returned an invalid result")?;
        value
            .get("delay")
            .and_then(Value::as_u64)
            .filter(|delay| *delay > 0)
            .context("The node test did not return a latency measurement")
    }

    pub async fn config_mode(&self) -> Result<String> {
        Ok(self.config_status().await?.mode)
    }

    pub async fn config_status(&self) -> Result<ConfigStatus> {
        let response = self.request(reqwest::Method::GET, "/configs").await?;
        if !response.status().is_success() {
            bail!("Mihomo controller rejected the configuration request");
        }
        let value: Value = response
            .json()
            .await
            .context("Mihomo configuration response was invalid")?;
        Ok(ConfigStatus {
            mode: value
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            tun_enabled: value.pointer("/tun/enable").and_then(Value::as_bool),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CoreController;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn node_test_uses_fixed_https_probe_and_returns_only_milliseconds() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 2048];
            let length = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..length]);
            assert!(request.starts_with(
                "GET /proxies/CF-safe/delay?url=https%3A%2F%2Fwww.gstatic.com%2Fgenerate_204&timeout=5000&expected=204 HTTP/1.1\r\n"
            ));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer fixture_secret")
            );
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\n{\"delay\":142}",
                )
                .await
                .unwrap();
        });
        let controller =
            CoreController::new(port, "fixture_secret".to_owned(), reqwest::Client::new());
        assert_eq!(controller.test_node_delay("CF-safe").await.unwrap(), 142);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn node_test_rejects_unsafe_core_names_without_a_request() {
        let controller = CoreController::new(1, "unused".to_owned(), reqwest::Client::new());
        assert!(
            controller
                .test_node_delay("CF-../../outside")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn node_test_does_not_return_controller_error_bodies() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 504 Gateway Timeout\r\nContent-Length: 28\r\nConnection: close\r\n\r\nfixture-private-error-detail",
                )
                .await
                .unwrap();
        });
        let controller =
            CoreController::new(port, "fixture_secret".to_owned(), reqwest::Client::new());
        let error = controller.test_node_delay("CF-safe").await.unwrap_err();
        assert!(!error.to_string().contains("fixture-private-error-detail"));
        server.await.unwrap();
    }
}
