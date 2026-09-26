use anyhow::{Context, Result, bail};
use rmcp::{
    RoleClient, ServiceExt,
    model::CallToolRequestParams,
    service::RunningService,
    transport::streamable_http_client::{
        StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    },
};
use serde_json::Value;
use std::time::Duration;

pub struct Client {
    service: RunningService<RoleClient, ()>,
    pub endpoint: String,
}

impl Client {
    pub async fn connect() -> Result<Self> {
        let endpoint = std::env::var("MORROWS_MCP_URL")
            .or_else(|_| std::env::var("AC_MCP_URL"))
            .unwrap_or_else(|_| "http://127.0.0.1:8787/mcp".into());
        let url = reqwest::Url::parse(&endpoint).context("invalid MORROWS_MCP_URL")?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            bail!(
                "MORROWS_MCP_URL must be an HTTP(S) endpoint without credentials, query or fragment"
            );
        }
        // Managed runtimes already carry their scoped Authorization value. Use
        // it before any inherited convenience token, never scan credential files.
        let token = match std::env::var("MORROWS_AGENT_AUTHORIZATION") {
            Ok(value) => value
                .strip_prefix("Bearer ")
                .context("MORROWS_AGENT_AUTHORIZATION must use Bearer authentication")?
                .to_owned(),
            Err(_) => std::env::var("MORROWS_AGENT_TOKEN")
                .context("set MORROWS_AGENT_TOKEN to an issued Agent Bearer credential")?,
        };
        if !token.starts_with("mrw_agent_") {
            bail!("MORROWS_AGENT_TOKEN must be an issued Agent credential");
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        let endpoint = url.to_string();
        let transport = StreamableHttpClientTransport::with_client(
            http,
            StreamableHttpClientTransportConfig::with_uri(endpoint.clone()).auth_header(token),
        );
        let service = tokio::time::timeout(Duration::from_secs(30), ().serve(transport))
            .await
            .context("Morrows connection timed out")?
            .context("connect to Morrows MCP")?;
        Ok(Self { service, endpoint })
    }

    pub async fn call(&self, name: &'static str, arguments: Value) -> Result<Value> {
        let result = tokio::time::timeout(
            Duration::from_secs(60),
            self.service.call_tool(
                CallToolRequestParams::new(name)
                    .with_arguments(arguments.as_object().context("tool arguments")?.clone()),
            ),
        )
        .await
        .context("Morrows request timed out; retry publications with the same idempotency key")??;
        let text = result
            .content
            .iter()
            .filter_map(|c| c.as_text())
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if result.is_error == Some(true) {
            bail!("{name}: {text}");
        }
        if let Some(value) = result.structured_content {
            return Ok(value);
        }
        serde_json::from_str(&text).with_context(|| format!("decode {name} result"))
    }
}
