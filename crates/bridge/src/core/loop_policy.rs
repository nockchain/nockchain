use std::time::Duration;

use backon::ExponentialBuilder;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    pub min_delay: Duration,
    pub max_delay: Duration,
    pub max_times: Option<usize>,
    pub jitter: bool,
}

impl RetryPolicy {
    pub fn exponential_builder(self) -> ExponentialBuilder {
        let builder = ExponentialBuilder::default()
            .with_min_delay(self.min_delay)
            .with_max_delay(self.max_delay);
        let builder = if self.jitter {
            builder.with_jitter()
        } else {
            builder
        };
        match self.max_times {
            Some(max_times) => builder.with_max_times(max_times),
            None => builder,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BaseObserverLoopPolicy {
    pub poll_interval: Duration,
    pub rpc_retry: RetryPolicy,
}

impl Default for BaseObserverLoopPolicy {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(30),
            rpc_retry: RetryPolicy {
                min_delay: Duration::from_secs(5),
                max_delay: Duration::from_secs(120),
                max_times: Some(10),
                jitter: true,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NockObserverLoopPolicy {
    pub poll_interval: Duration,
    pub connect_retry: RetryPolicy,
    pub connect_failure_sleep: Duration,
}

impl Default for NockObserverLoopPolicy {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(10),
            connect_retry: RetryPolicy {
                min_delay: Duration::from_secs(1),
                max_delay: Duration::from_secs(300),
                max_times: None,
                jitter: true,
            },
            connect_failure_sleep: Duration::from_secs(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigningLoopPolicy {
    pub poll_interval: Duration,
    pub pipeline_depth: usize,
    pub regossip_interval: Duration,
}

impl Default for SigningLoopPolicy {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(15),
            pipeline_depth: 4,
            regossip_interval: Duration::from_secs(90),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostingLoopPolicy {
    pub tick_interval: Duration,
    pub failover_backoff_secs: u64,
}

impl Default for PostingLoopPolicy {
    fn default() -> Self {
        Self {
            tick_interval: Duration::from_secs(1),
            failover_backoff_secs: 120,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_base_observer_policy_matches_existing_behavior() {
        let policy = BaseObserverLoopPolicy::default();
        assert_eq!(policy.poll_interval, Duration::from_secs(30));
        assert_eq!(policy.rpc_retry.min_delay, Duration::from_secs(5));
        assert_eq!(policy.rpc_retry.max_delay, Duration::from_secs(120));
        assert_eq!(policy.rpc_retry.max_times, Some(10));
        assert!(policy.rpc_retry.jitter);
    }

    #[test]
    fn default_nock_observer_policy_matches_existing_behavior() {
        let policy = NockObserverLoopPolicy::default();
        assert_eq!(policy.poll_interval, Duration::from_secs(10));
        assert_eq!(policy.connect_retry.min_delay, Duration::from_secs(1));
        assert_eq!(policy.connect_retry.max_delay, Duration::from_secs(300));
        assert_eq!(policy.connect_retry.max_times, None);
        assert!(policy.connect_retry.jitter);
        assert_eq!(policy.connect_failure_sleep, Duration::from_secs(1));
    }

    #[test]
    fn default_signing_and_posting_policies_match_existing_behavior() {
        let signing = SigningLoopPolicy::default();
        assert_eq!(signing.poll_interval, Duration::from_secs(15));
        assert_eq!(signing.pipeline_depth, 4);
        assert_eq!(signing.regossip_interval, Duration::from_secs(90));

        let posting = PostingLoopPolicy::default();
        assert_eq!(posting.tick_interval, Duration::from_secs(1));
        assert_eq!(posting.failover_backoff_secs, 120);
    }
}
