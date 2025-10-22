use crate::Error;
use nostr::prelude::RelayUrl;
use nostr_sdk::prelude::{Client, Event, Filter, Keys, ReceiverStream};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Duration;
use tracing::{debug, warn};

/// Persistent relay pool responsible for maintaining long-lived connections.
#[derive(Clone)]
pub struct RelayPool {
    client: Client,
    known_relays: Arc<Mutex<HashSet<String>>>,
    default_relays: Arc<[RelayUrl]>,
    connect_timeout: Duration,
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
        let mut new_relays = Vec::new();
        let mut had_new = false;
        {
            let mut guard = self.known_relays.lock().await;
            for relay in relays {
                let key = relay.to_string();
                if guard.insert(key) {
                    new_relays.push(relay);
                    had_new = true;
                }
            }
        }

        for relay in new_relays {
            debug!("adding relay {}", relay);
            self.client
                .add_relay(relay.clone())
                .await
                .map_err(|err| Error::Generic(format!("failed to add relay {relay}: {err}")))?;
            if let Err(err) = self.client.connect_relay(relay.clone()).await {
                warn!("failed to connect relay {}: {}", relay, err);
            }
        }

        if had_new {
            self.client.connect_with_timeout(self.connect_timeout).await;
        }

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
}
