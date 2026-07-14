use serde_json::{Value, json};

/// A single route entry for the Caddy config.
pub struct CaddyRoute {
  pub hostname: String,
  pub target_endpoint_id: String,
  pub target_port: u16,
}

/// Build the complete Caddy JSON config from a list of routes.
/// This is POSTed to Caddy's admin API at /load.
pub fn build_caddy_config(
  routes: &[CaddyRoute],
  cloudflare_api_token: &str,
  bridge_port: u16,
) -> Value {
  let route_entries: Vec<Value> = routes
    .iter()
    .map(|route| {
      json!({
        "match": [{
          "host": [route.hostname]
        }],
        "handle": [{
          "handler": "reverse_proxy",
          "upstreams": [{
            "dial": format!("127.0.0.1:{bridge_port}")
          }],
          "headers": {
            "request": {
              "set": {
                "X-Target-Endpoint": [route.target_endpoint_id],
                "X-Target-Port": [route.target_port.to_string()]
              }
            }
          }
        }]
      })
    })
    .collect();

  json!({
    "apps": {
      "http": {
        "servers": {
          "main": {
            "listen": [":80", ":443"],
            "automatic_https": {
              "disable_redirects": false
            },
            "routes": route_entries
          }
        }
      },
      "tls": {
        "automation": {
          "policies": [{
            "issuers": [{
              "module": "acme",
              "challenges": {
                "dns": {
                  "provider": {
                    "name": "cloudflare",
                    "api_token": cloudflare_api_token
                  }
                }
              }
            }]
          }]
        }
      }
    }
  })
}
