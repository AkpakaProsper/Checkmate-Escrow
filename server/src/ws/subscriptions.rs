use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

/// Tracks which clients are subscribed to which match_id so that events are
/// only dispatched to the clients actually watching that match.
#[derive(Default)]
pub struct SubscriptionMap {
    /// match_id -> set of subscribed client ids
    by_match: RwLock<HashMap<String, HashSet<String>>>,
    /// client id -> set of match_ids the client is subscribed to
    by_client: RwLock<HashMap<String, HashSet<String>>>,
}

impl SubscriptionMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe `client` to `match_id`. Idempotent.
    pub fn subscribe(&self, match_id: &str, client: &str) {
        {
            let mut by_match = self.by_match.write().unwrap();
            by_match
                .entry(match_id.to_string())
                .or_default()
                .insert(client.to_string());
        }
        let mut by_client = self.by_client.write().unwrap();
        by_client
            .entry(client.to_string())
            .or_default()
            .insert(match_id.to_string());
    }

    /// Remove `client` from `match_id`. Idempotent.
    pub fn unsubscribe(&self, match_id: &str, client: &str) {
        {
            let mut by_match = self.by_match.write().unwrap();
            if let Some(clients) = by_match.get_mut(match_id) {
                clients.remove(client);
                if clients.is_empty() {
                    by_match.remove(match_id);
                }
            }
        }
        let mut by_client = self.by_client.write().unwrap();
        if let Some(matches) = by_client.get_mut(client) {
            matches.remove(match_id);
            if matches.is_empty() {
                by_client.remove(client);
            }
        }
    }

    /// Remove all subscriptions for a disconnected client.
    pub fn remove_client(&self, client: &str) {
        let matches = {
            let mut by_client = self.by_client.write().unwrap();
            by_client.remove(client).unwrap_or_default()
        };
        let mut by_match = self.by_match.write().unwrap();
        for match_id in matches {
            if let Some(clients) = by_match.get_mut(&match_id) {
                clients.remove(client);
                if clients.is_empty() {
                    by_match.remove(&match_id);
                }
            }
        }
    }

    /// Return the clients subscribed to `match_id`.
    pub fn subscribers(&self, match_id: &str) -> Vec<String> {
        let by_match = self.by_match.read().unwrap();
        by_match
            .get(match_id)
            .map(|clients| clients.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Return the match_ids a client is subscribed to.
    pub fn subscriptions(&self, client: &str) -> Vec<String> {
        let by_client = self.by_client.read().unwrap();
        by_client
            .get(client)
            .map(|matches| matches.iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_only_to_subscribers_of_match() {
        let subs = SubscriptionMap::new();
        subs.subscribe("match-a", "client-1");
        subs.subscribe("match-b", "client-2");

        assert_eq!(subs.subscribers("match-a"), vec!["client-1".to_string()]);
        assert_eq!(subs.subscribers("match-b"), vec!["client-2".to_string()]);
        assert!(subs.subscribers("match-c").is_empty());
    }

    #[test]
    fn two_clients_different_matches_are_isolated() {
        let subs = SubscriptionMap::new();
        subs.subscribe("match-a", "client-1");
        subs.subscribe("match-b", "client-2");

        let a = subs.subscribers("match-a");
        let b = subs.subscribers("match-b");

        assert!(a.contains(&"client-1".to_string()));
        assert!(!a.contains(&"client-2".to_string()));
        assert!(b.contains(&"client-2".to_string()));
        assert!(!b.contains(&"client-1".to_string()));
    }

    #[test]
    fn unsubscribe_and_remove_client_clean_up() {
        let subs = SubscriptionMap::new();
        subs.subscribe("match-a", "client-1");
        subs.subscribe("match-b", "client-1");

        subs.unsubscribe("match-a", "client-1");
        assert!(subs.subscribers("match-a").is_empty());
        assert_eq!(subs.subscribers("match-b"), vec!["client-1".to_string()]);

        subs.remove_client("client-1");
        assert!(subs.subscribers("match-b").is_empty());
        assert!(subs.subscriptions("client-1").is_empty());
    }
}
