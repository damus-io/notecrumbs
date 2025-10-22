use crate::Error;
use nostr::prelude::RelayUrl;
use nostr_sdk::prelude::{Client, Event, Filter, Keys, ReceiverStream};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Duration;
use tracing::{debug, info, warn};

#[derive(Clone, Copy, Debug, Default)]
pub struct RelayStats {
    pub ensure_calls: u64,
    pub relays_added: u64,
    pub connect_successes: u64,
    pub connect_failures: u64,
}

/// Persistent relay pool responsible for maintaining long-lived connections.
#[derive(Clone)]
pub struct RelayPool {
    client: Client,
    known_relays: Arc<Mutex<HashSet<String>>>,
    default_relays: Arc<[RelayUrl]>,
    connect_timeout: Duration,
    stats: Arc<Mutex<RelayStats>>,
}

impl RelayPool {
    pub async fn new(
        keys: Keys,
        default_relays: &[&str],
        connect_timeout: Duration,
    ) -> Result<Self, Error> {
        let client = Client::builder().signer(keys).build();
        let parsed_defaults: Vec<RelayUrl> = default_relays
            .iter()
            .filter_map(|url| match RelayUrl::parse(url) {
                Ok(relay) => Some(relay),
                Err(err) => {
                    warn!("failed to parse default relay {url}: {err}");
                    None
                }
            })
            .collect();

        let default_relays = Arc::<[RelayUrl]>::from(parsed_defaults);
        let pool = Self {
            client,
            known_relays: Arc::new(Mutex::new(HashSet::new())),
            default_relays: default_relays.clone(),
            connect_timeout,
            stats: Arc::new(Mutex::new(RelayStats::default())),
        };

        pool.ensure_relays(pool.default_relays().iter().cloned())
            .await?;

        Ok(pool)
    }

    pub fn default_relays(&self) -> &[RelayUrl] {
        self.default_relays.as_ref()
    }

    pub async fn ensure_relays<I>(&self, relays: I) -> Result<(), Error>
    where
        I: IntoIterator<Item = RelayUrl>,
    {
        metrics::counter!("relay_pool_ensure_calls_total", 1);
        let mut new_relays = Vec::new();
        let mut had_new = false;
        let mut relays_added = 0u64;
        {
            let mut guard = self.known_relays.lock().await;
            for relay in relays {
                let key = relay.to_string();
                if guard.insert(key) {
                    new_relays.push(relay);
                    had_new = true;
                    relays_added += 1;
                }
            }
        }

        if relays_added > 0 {
            metrics::counter!("relay_pool_relays_added_total", relays_added);
        }

        let mut connect_success = 0u64;
        let mut connect_failure = 0u64;
        for relay in new_relays {
            debug!("adding relay {}", relay);
            self.client
                .add_relay(relay.clone())
                .await
                .map_err(|err| Error::Generic(format!("failed to add relay {relay}: {err}")))?;
            if let Err(err) = self.client.connect_relay(relay.clone()).await {
                warn!("failed to connect relay {}: {}", relay, err);
                connect_failure += 1;
            } else {
                connect_success += 1;
            }
        }

        if connect_success > 0 {
            metrics::counter!("relay_pool_connect_success_total", connect_success);
        }
        if connect_failure > 0 {
            metrics::counter!("relay_pool_connect_failure_total", connect_failure);
        }

        if had_new {
            self.client.connect_with_timeout(self.connect_timeout).await;

            let mut stats = self.stats.lock().await;
            stats.ensure_calls += 1;
            stats.relays_added += relays_added;
            stats.connect_successes += connect_success;
            stats.connect_failures += connect_failure;
            let snapshot = *stats;
            drop(stats);

            let tracked = {
                let guard = self.known_relays.lock().await;
                guard.len()
            };

            info!(
                total_relays = tracked,
                ensure_calls = snapshot.ensure_calls,
                relays_added = relays_added,
                connect_successes = connect_success,
                connect_failures = connect_failure,
                "relay pool health update"
            );
        } else {
            let mut stats = self.stats.lock().await;
            stats.ensure_calls += 1;
        }

        let tracked = {
            let guard = self.known_relays.lock().await;
            guard.len()
        };
        metrics::gauge!("relay_pool_known_relays", tracked as f64);

        Ok(())
    }

    pub async fn stream_events(
        &self,
        filters: Vec<Filter>,
        relays: &[RelayUrl],
        timeout: Duration,
    ) -> Result<ReceiverStream<Event>, Error> {
        if relays.is_empty() {
            Ok(self.client.stream_events(filters, Some(timeout)).await?)
        } else {
            let urls: Vec<String> = relays.iter().map(|r| r.to_string()).collect();
            Ok(self
                .client
                .stream_events_from(urls, filters, Some(timeout))
                .await?)
        }
    }

    pub async fn relay_stats(&self) -> (RelayStats, usize) {
        let stats = { *self.stats.lock().await };
        let tracked = {
            let guard = self.known_relays.lock().await;
            guard.len()
        };
        (stats, tracked)
    }
}
