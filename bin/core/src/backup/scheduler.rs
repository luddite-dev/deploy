use std::str::FromStr;
use std::sync::OnceLock;

use anyhow::Context;
use croner::parser::{CronParser, Seconds};
use croner::Cron;
use database::mungos::{find::find_collect, mongodb::bson::doc};
use komodo_client::entities::deployment::Deployment;
use komodo_client::entities::stack::Stack;
use tracing::warn;

use crate::state::db_client;

/// Background loop: tick every 60s, firing any backup whose cron
/// schedule is due within the last tick window.
pub async fn run_scheduler() {
  loop {
    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    if let Err(e) = tick().await {
      warn!("Backup scheduler tick failed: {e:#}");
    }
  }
}

async fn tick() -> anyhow::Result<()> {
  let now = chrono::Utc::now();

  let deployments: Vec<Deployment> =
    find_collect(&db_client().deployments, doc! {}, None)
      .await
      .context("Failed to query deployments")?;

  for deployment in deployments {
    let Some(backup_config) = &deployment.config.backup else {
      continue;
    };
    let Some(cron_expr) = &backup_config.schedule else {
      continue;
    };
    if deployment.info.migration_state.is_some() {
      continue;
    }

    let schedule = match parse_cron(cron_expr) {
      Ok(Some(s)) => s,
      Ok(None) => continue,
      Err(e) => {
        warn!(
          "Invalid cron for deployment {}: {e}",
          deployment.id
        );
        continue;
      }
    };

    if should_fire(&schedule, &now) {
      if let Err(e) =
        super::backup_deployment_volumes(&deployment.id).await
      {
        warn!(
          "Scheduled backup failed for deployment {}: {e:#}",
          deployment.id
        );
      }
    }
  }

  let stacks: Vec<Stack> =
    find_collect(&db_client().stacks, doc! {}, None)
      .await
      .context("Failed to query stacks")?;

  for stack in stacks {
    let Some(backup_config) = &stack.config.backup else {
      continue;
    };
    let Some(cron_expr) = &backup_config.schedule else {
      continue;
    };
    if stack.info.migration_state.is_some() {
      continue;
    }

    let schedule = match parse_cron(cron_expr) {
      Ok(Some(s)) => s,
      Ok(None) => continue,
      Err(e) => {
        warn!("Invalid cron for stack {}: {e}", stack.id);
        continue;
      }
    };

    if should_fire(&schedule, &now) {
      if let Err(e) = super::backup_stack_volumes(&stack.id).await {
        warn!(
          "Scheduled backup failed for stack {}: {e:#}",
          stack.id
        );
      }
    }
  }

  Ok(())
}

/// Shared cron parser. Mirrors schedule.rs: 6-field expressions with
/// required seconds, and DOM-and-DOW intersection semantics.
fn cron_parser() -> &'static CronParser {
  static CRON_PARSER: OnceLock<CronParser> = OnceLock::new();
  CRON_PARSER.get_or_init(|| {
    CronParser::builder()
      .seconds(Seconds::Required)
      .dom_and_dow(true)
      .build()
  })
}

fn parse_cron(expr: &str) -> anyhow::Result<Option<Cron>> {
  // Empty schedule => on-demand only, nothing to schedule.
  if expr.trim().is_empty() {
    return Ok(None);
  }
  let cron = cron_parser()
    .parse(expr)
    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
  Ok(Some(cron))
}

/// Fire if the most recent scheduled time before `now` fell within the
/// last 60s. The scheduler ticks every 60s, so each scheduled fire is
/// caught within exactly one tick window.
fn should_fire(
  schedule: &Cron,
  now: &chrono::DateTime<chrono::Utc>,
) -> bool {
  match schedule.find_previous_occurrence(now, false) {
    Ok(last) => {
      now.signed_duration_since(last).num_seconds() < 60
    }
    Err(_) => false,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use chrono::TimeZone;

  #[test]
  fn test_shoulfire_within_window() {
    // "every second" — previous occurrence is one second ago.
    let cron = cron_parser().parse("* * * * * *").unwrap();
    let now = chrono::Utc.with_ymd_and_hms(2026, 6, 25, 12, 0, 30).unwrap();
    assert!(should_fire(&cron, &now));
  }

  #[test]
  fn test_shoulfire_outside_window() {
    // "top of every hour" — at :00:05 the previous fire was 5s ago (<60, fire).
    // At :01:05 the previous fire was 65s ago (>=60, no fire).
    let cron = cron_parser().parse("0 0 * * * *").unwrap();
    let fire = chrono::Utc.with_ymd_and_hms(2026, 6, 25, 12, 0, 5).unwrap();
    assert!(should_fire(&cron, &fire), "5s after :00:00 should fire");
    let no_fire =
      chrono::Utc.with_ymd_and_hms(2026, 6, 25, 12, 1, 5).unwrap();
    assert!(
      !should_fire(&cron, &no_fire),
      "65s after :00:00 should not fire"
    );
  }

  #[test]
  fn test_parse_cron_empty_is_none() {
    assert!(parse_cron("").unwrap().is_none());
    assert!(parse_cron("   ").unwrap().is_none());
  }

  #[test]
  fn test_parse_cron_valid() {
    assert!(parse_cron("0 0 3 * * *").unwrap().is_some());
  }

  #[test]
  fn test_parse_cron_invalid_errors() {
    assert!(parse_cron("not a cron").is_err());
  }
}
