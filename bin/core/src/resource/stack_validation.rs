use serde_yaml_ng::Value;
use anyhow::{bail, Context};

/// Validate a compose file YAML string.
/// Rejects:
/// - Bind mounts (host paths in service volumes)
/// - Swarm-only compose keys (deploy, replicas, placement, etc.)
pub fn validate_compose_yaml(yaml: &str) -> anyhow::Result<()> {
  let parsed: Value = serde_yaml_ng::from_str(yaml)
    .context("Failed to parse compose file as YAML")?;

  let services = parsed
    .get("services")
    .and_then(|s| s.as_mapping())
    .context("Compose file must have a 'services' key")?;

  let declared_volumes: std::collections::HashSet<String> = parsed
    .get("volumes")
    .and_then(|v| v.as_mapping())
    .map(|m| m.keys().filter_map(|k| k.as_str().map(String::from)).collect())
    .unwrap_or_default();

  for (service_name, service_val) in services {
    let service_name = service_name.as_str().unwrap_or("unknown");

    if service_val.get("deploy").is_some() {
      bail!(
        "Service '{service_name}' uses 'deploy' key, which is Swarm-only. \
         Swarm mode has been removed from this fork."
      );
    }

    if let Some(volumes) = service_val.get("volumes").and_then(|v| v.as_sequence()) {
      for vol in volumes {
        let vol_str = match vol {
          Value::String(s) => s.clone(),
          Value::Mapping(m) => {
            if let Some(t) = m.get("type").and_then(|v| v.as_str()) {
              if t == "bind" {
                let source = m.get("source").and_then(|v| v.as_str()).unwrap_or("?");
                bail!(
                  "Service '{service_name}' has a bind mount (source: '{source}'). \
                   Only named volumes are allowed."
                );
              }
              continue;
            }
            let source = m.get("source").and_then(|v| v.as_str());
            let target = m.get("target").and_then(|v| v.as_str());
            if let (Some(src), Some(_tgt)) = (source, target) {
              if !declared_volumes.contains(src) {
                bail!(
                  "Service '{service_name}' mounts volume '{src}' which is not \
                   declared in the top-level volumes section. Bind mounts are not allowed."
                );
              }
            }
            continue;
          }
          _ => continue,
        };

        let parts: Vec<&str> = vol_str.splitn(3, ':').collect();
        if parts.len() >= 2 {
          let source = parts[0];
          if !source.is_empty() && !declared_volumes.contains(source) {
            bail!(
              "Service '{service_name}' has volume '{source}' which is not \
               declared in the top-level volumes section. Bind mounts are not allowed."
            );
          }
        }
      }
    }
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_valid_named_volumes() {
    let yaml = r#"
services:
  web:
    image: nginx
    volumes:
      - data:/var/lib/data
volumes:
  data:
"#;
    assert!(validate_compose_yaml(yaml).is_ok());
  }

  #[test]
  fn test_reject_bind_mount() {
    let yaml = r#"
services:
  web:
    image: nginx
    volumes:
      - /host/path:/container/path
"#;
    let result = validate_compose_yaml(yaml);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not declared"));
  }

  #[test]
  fn test_reject_deploy_key() {
    let yaml = r#"
services:
  web:
    image: nginx
    deploy:
      replicas: 3
"#;
    let result = validate_compose_yaml(yaml);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Swarm-only"));
  }

  #[test]
  fn test_reject_long_form_bind_mount() {
    let yaml = r#"
services:
  web:
    image: nginx
    volumes:
      - type: bind
        source: /host/path
        target: /data
"#;
    let result = validate_compose_yaml(yaml);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("bind mount"));
  }

  #[test]
  fn test_anonymous_volume_ok() {
    let yaml = r#"
services:
  web:
    image: nginx
    volumes:
      - /var/lib/data
"#;
    assert!(validate_compose_yaml(yaml).is_ok());
  }
}
