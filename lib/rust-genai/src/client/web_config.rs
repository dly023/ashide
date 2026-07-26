use std::time::Duration;

/// Reqwest client configuration.
///
/// By default enables performance optimizations:
/// - TCP_NODELAY (lower latency, disables Nagle's algorithm)
/// - HTTP/2 keep-alive (prevents idle connection drops)
/// - HTTP/2 adaptive flow-control window
/// - Connection pool: 4 idle connections per host
///
/// **Note (Ashide fork)**: gzip default is **false**. Upstream genai defaults
/// to `true`, but for AI streaming endpoints (Anthropic / OpenAI compatible)
/// `Accept-Encoding: gzip` causes proxies (nginx with `gzip on; gzip_proxied any;`)
/// to compress SSE responses. SSE over gzip forces the server to flush full
/// deflate frames before the client can decode any text → stream becomes ~K-byte
/// bursts every ~400ms instead of token-level streaming. zed/opencode don't
/// negotiate gzip on SSE so they don't hit this. Bandwidth impact of disabling
/// gzip on SSE is negligible (per-line small JSON is poorly compressible) but
/// the streaming UX improvement is dramatic.
#[derive(Debug, Clone)]
pub struct WebConfig {
	pub timeout: Option<Duration>,
	pub connect_timeout: Option<Duration>,
	pub read_timeout: Option<Duration>,
	pub default_headers: Option<reqwest::header::HeaderMap>,
	pub proxy: Option<reqwest::Proxy>,
	/// When true, disable automatic proxy discovery (system proxy, env vars).
	/// Calls `reqwest::ClientBuilder::no_proxy()`. If an explicit `proxy` is
	/// also set on this config, the explicit proxy still takes effect.
	/// Default: false.
	pub no_proxy: bool,
	/// Enable gzip response decompression. **Default: false** (Ashide fork
	/// — upstream genai default is true). See struct-level docs for rationale.
	pub gzip: bool,
	/// Enable TCP_NODELAY (disable Nagle's algorithm). Default: true.
	pub tcp_nodelay: bool,
}

impl Default for WebConfig {
	fn default() -> Self {
		Self {
			timeout: None,
			connect_timeout: None,
			read_timeout: None,
			default_headers: None,
			proxy: None,
			no_proxy: false,
			// Ashide: gzip off by default — see struct-level docs above.
			gzip: false,
			tcp_nodelay: true,
		}
	}
}

impl WebConfig {
	/// Sets the per-request timeout.
	pub fn with_timeout(mut self, timeout: Duration) -> Self {
		self.timeout = Some(timeout);
		self
	}

	/// Sets the connect timeout.
	pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
		self.connect_timeout = Some(timeout);
		self
	}

	/// Sets default headers.
	pub fn with_default_headers(mut self, headers: reqwest::header::HeaderMap) -> Self {
		self.default_headers = Some(headers);
		self
	}

	/// Sets the proxy.
	pub fn with_proxy(mut self, proxy: reqwest::Proxy) -> Self {
		self.proxy = Some(proxy);
		self
	}

	/// Sets the HTTP proxy from a URL.
	pub fn with_proxy_url(mut self, proxy_url: &str) -> Result<Self, reqwest::Error> {
		let proxy = reqwest::Proxy::http(proxy_url)?;
		self.proxy = Some(proxy);
		Ok(self)
	}

	/// Sets the HTTPS proxy from a URL.
	pub fn with_https_proxy_url(mut self, proxy_url: &str) -> Result<Self, reqwest::Error> {
		let proxy = reqwest::Proxy::https(proxy_url)?;
		self.proxy = Some(proxy);
		Ok(self)
	}

	/// Sets the proxy for all schemes from a URL.
	pub fn with_all_proxy_url(mut self, proxy_url: &str) -> Result<Self, reqwest::Error> {
		let proxy = reqwest::Proxy::all(proxy_url)?;
		self.proxy = Some(proxy);
		Ok(self)
	}

	/// Sets the proxy for all schemes, including optional basic auth and no_proxy rules.
	pub fn set_proxy_settings(
		&mut self,
		proxy_url: &str,
		username: &str,
		password: &str,
		no_proxy: &str,
	) -> Result<(), reqwest::Error> {
		let mut proxy = reqwest::Proxy::all(proxy_url)?;

		if !username.is_empty() || !password.is_empty() {
			proxy = proxy.basic_auth(username, password);
		}

		let no_proxy = no_proxy.trim();
		if !no_proxy.is_empty() {
			if let Some(no_proxy) = reqwest::NoProxy::from_string(no_proxy) {
				proxy = proxy.no_proxy(Some(no_proxy));
			}
		}

		self.proxy = Some(proxy);
		Ok(())
	}

	/// Applies this config to a reqwest::ClientBuilder.
	pub fn apply_to_builder(&self, mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
		if let Some(timeout) = self.timeout {
			builder = builder.timeout(timeout);
		}
		if let Some(connect_timeout) = self.connect_timeout {
			builder = builder.connect_timeout(connect_timeout);
		}
		if let Some(read_timeout) = self.read_timeout {
			builder = builder.read_timeout(read_timeout);
		}
		if let Some(ref headers) = self.default_headers {
			builder = builder.default_headers(headers.clone());
		}
		if self.no_proxy {
			builder = builder.no_proxy();
		}
		if let Some(ref proxy) = self.proxy {
			builder = builder.proxy(proxy.clone());
		}
		// Performance optimizations
		if self.gzip {
			builder = builder.gzip(true);
		}
		if self.tcp_nodelay {
			builder = builder.tcp_nodelay(true);
		}
		// HTTP/2 connection tuning
		builder = builder
			.pool_max_idle_per_host(4)
			.http2_keep_alive_interval(Some(Duration::from_secs(20)))
			.http2_keep_alive_timeout(Duration::from_secs(10))
			.http2_keep_alive_while_idle(true)
			.http2_adaptive_window(true);
		builder
	}
}
