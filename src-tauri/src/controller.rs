use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::{Value, json};

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

    pub async fn version(&self) -> Result<Value> {
        let response = self.request(reqwest::Method::GET, "/version").await?;
        if !response.status().is_success() {
            bail!("Mihomo controller rejected the version request");
        }
        response
            .json()
            .await
            .context("Mihomo version response was invalid")
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

    pub async fn config_mode(&self) -> Result<String> {
        let response = self.request(reqwest::Method::GET, "/configs").await?;
        if !response.status().is_success() {
            bail!("Mihomo controller rejected the configuration request");
        }
        let value: Value = response
            .json()
            .await
            .context("Mihomo configuration response was invalid")?;
        Ok(value
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned())
    }
}
