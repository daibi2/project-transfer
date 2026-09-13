use crate::envvar;
use url::Url;

pub const ENV_API_BASE_URL: &str = envvar::ENV_AGENT_STUB_API_BASE_URL;
pub const ENV_AUTH_JWE: &str = envvar::ENV_AGENT_STUB_AUTH_JWE;

#[derive(Debug, Clone)]
pub struct Environment {
    pub url: String,
    pub auth_jwe: String,
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub url: String,
}

pub fn read_environment() -> Result<Environment, String> {
    let api_url = std::env::var(ENV_API_BASE_URL)
        .unwrap_or_default()
        .trim()
        .to_string();
    let auth_jwe = std::env::var(ENV_AUTH_JWE)
        .unwrap_or_default()
        .trim()
        .to_string();
    let mut missing = Vec::new();
    if api_url.is_empty() {
        missing.push(ENV_API_BASE_URL);
    }
    if auth_jwe.is_empty() {
        missing.push(ENV_AUTH_JWE);
    }
    if !missing.is_empty() {
        return Err(format!(
            "missing required Agent Stub environment variables: {}",
            missing.join(", ")
        ));
    }
    let endpoint = parse_endpoint(&api_url)
        .map_err(|e| format!("invalid {ENV_API_BASE_URL}: {e}"))?;
    Ok(Environment {
        url: endpoint.url,
        auth_jwe,
    })
}

pub fn has_environment() -> bool {
    !std::env::var(ENV_API_BASE_URL).unwrap_or_default().is_empty()
        && !std::env::var(ENV_AUTH_JWE).unwrap_or_default().is_empty()
}

pub fn parse_endpoint(raw_url: &str) -> Result<Endpoint, String> {
    let stripped = raw_url.trim();
    if stripped.is_empty() {
        return Err("agent stub URL must not be empty".into());
    }
    let parsed = Url::parse(stripped).map_err(|e| format!("invalid URL: {e}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("agent stub URL must use http or https".into());
    }
    parse_http_endpoint(&parsed)
}

fn parse_http_endpoint(parsed: &Url) -> Result<Endpoint, String> {
    if parsed.host_str().is_none() || parsed.host_str() == Some("") {
        return Err("agent stub URL must include a host".into());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("agent stub URL must not include a query string or fragment".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("agent stub URL must not include user info".into());
    }
    let mut path = parsed.path().trim_end_matches('/').to_string();
    if path.is_empty() || path == "/" {
        path = "/agent-stub".into();
    } else if path != "/agent-stub" {
        return Err("HTTP agent stub API base URL path must be empty or /agent-stub".into());
    }
    Ok(Endpoint {
        url: format!("{}://{}{path}", parsed.scheme(), host_port(parsed)),
    })
}

fn host_port(parsed: &Url) -> String {
    match parsed.port() {
        Some(p) => format!("{}:{p}", parsed.host_str().unwrap_or("")),
        None => parsed.host_str().unwrap_or("").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_http() {
        assert_eq!(
            parse_endpoint("http://localhost:8080").unwrap().url,
            "http://localhost:8080/agent-stub"
        );
        assert_eq!(
            parse_endpoint("https://agent.example.com").unwrap().url,
            "https://agent.example.com/agent-stub"
        );
        assert_eq!(
            parse_endpoint("http://localhost:8080/agent-stub").unwrap().url,
            "http://localhost:8080/agent-stub"
        );
        assert_eq!(
            parse_endpoint("http://localhost:8080/").unwrap().url,
            "http://localhost:8080/agent-stub"
        );
        assert!(parse_endpoint("http://localhost:8080/other-path").is_err());
        assert!(parse_endpoint("http://localhost:8080?foo=bar").is_err());
        assert!(parse_endpoint("").is_err());
        assert!(parse_endpoint("ftp://example.com").is_err());
    }
}
