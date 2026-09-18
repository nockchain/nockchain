use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

#[cfg(test)]
pub fn channel<K: Eq + Hash, V>() -> (Sender<K, V>, Receiver<K, V>) {
    channel_with_limits(usize::MAX, usize::MAX)
}

pub fn channel_with_limits<K: Eq + Hash, V>(
    max_total: usize,
    max_per_key: usize,
) -> (Sender<K, V>, Receiver<K, V>) {
    let (key_sender, key_receiver) = mpsc::unbounded_channel();
    let enqueued = Arc::new(Mutex::new(QueueState {
        values: HashMap::new(),
        total_len: 0,
    }));
    let limits = QueueLimits {
        max_total,
        max_per_key,
    };
    let sender = Sender {
        enqueued: enqueued.clone(),
        key_sender: key_sender.clone(),
        limits,
    };
    let receiver = Receiver {
        enqueued,
        key_receiver,
        key_sender,
    };
    (sender, receiver)
}

struct QueueState<K, V> {
    values: HashMap<K, VecDeque<V>>,
    total_len: usize,
}

#[derive(Clone, Copy)]
struct QueueLimits {
    max_total: usize,
    max_per_key: usize,
}

pub struct Sender<K, V> {
    enqueued: Arc<Mutex<QueueState<K, V>>>,
    key_sender: mpsc::UnboundedSender<K>,
    limits: QueueLimits,
}

impl<K, V> Clone for Sender<K, V> {
    fn clone(&self) -> Self {
        Sender {
            enqueued: self.enqueued.clone(),
            key_sender: self.key_sender.clone(),
            limits: self.limits,
        }
    }
}

pub struct Receiver<K, V> {
    enqueued: Arc<Mutex<QueueState<K, V>>>,
    key_receiver: mpsc::UnboundedReceiver<K>,
    key_sender: mpsc::UnboundedSender<K>,
}

#[derive(Debug)]
pub enum Error<K> {
    SendError(mpsc::error::SendError<K>),
    Full,
}

impl<K> From<mpsc::error::SendError<K>> for Error<K> {
    fn from(err: mpsc::error::SendError<K>) -> Self {
        Error::SendError(err)
    }
}

impl<K: Eq + Hash + Clone, V> Sender<K, V> {
    pub fn send(&self, key: K, value: V) -> Result<(), Error<K>> {
        let should_schedule = {
            let mut enqueued = self
                .enqueued
                .lock()
                .expect("key_fair_queue sender lock should not be poisoned");
            if enqueued.total_len >= self.limits.max_total {
                return Err(Error::Full);
            }
            let queue = enqueued.values.entry(key.clone()).or_default();
            if queue.len() >= self.limits.max_per_key {
                return Err(Error::Full);
            }
            let should_schedule = queue.is_empty();
            queue.push_back(value);
            enqueued.total_len = enqueued.total_len.saturating_add(1);
            should_schedule
        };

        if should_schedule {
            self.key_sender.send(key)?;
        }

        Ok(())
    }
}

impl<K: Eq + Hash + Clone, V> Receiver<K, V> {
    pub async fn recv(&mut self) -> Option<(K, V)> {
        let key = self.key_receiver.recv().await?;
        let (value, has_more) = {
            let mut enqueued = self
                .enqueued
                .lock()
                .expect("key_fair_queue receiver lock should not be poisoned");
            let queue = enqueued
                .values
                .get_mut(&key)
                .expect("Key from queue should be in map");
            let value = queue
                .pop_front()
                .expect("Key from queue should have a pending value");
            let has_more = !queue.is_empty();
            if !has_more {
                enqueued.values.remove(&key);
            }
            enqueued.total_len = enqueued.total_len.saturating_sub(1);
            (value, has_more)
        };

        if has_more {
            self.key_sender
                .send(key.clone())
                .expect("Receiver should be alive when requeueing a pending key");
        }

        Some((key, value))
    }
}

#[cfg(test)]
mod tests {
    use super::{channel, channel_with_limits, Error};

    #[tokio::test]
    async fn preserves_multiple_values_for_same_key() {
        let (sender, mut receiver) = channel::<u8, u8>();

        sender.send(7, 1).expect("first send should succeed");
        sender.send(7, 2).expect("second send should succeed");

        assert_eq!(receiver.recv().await, Some((7, 1)));
        assert_eq!(receiver.recv().await, Some((7, 2)));
    }

    #[tokio::test]
    async fn round_robins_pending_keys() {
        let (sender, mut receiver) = channel::<u8, u8>();

        sender.send(1, 10).expect("first send should succeed");
        sender.send(1, 11).expect("second send should succeed");
        sender.send(2, 20).expect("third send should succeed");

        assert_eq!(receiver.recv().await, Some((1, 10)));
        assert_eq!(receiver.recv().await, Some((2, 20)));
        assert_eq!(receiver.recv().await, Some((1, 11)));
    }

    #[tokio::test]
    async fn bounded_channel_rejects_per_key_overflow() {
        let (sender, mut receiver) = channel_with_limits::<u8, u8>(4, 1);

        sender.send(1, 10).expect("first send should fit");
        assert!(matches!(sender.send(1, 11), Err(Error::Full)));
        assert_eq!(receiver.recv().await, Some((1, 10)));
    }

    #[tokio::test]
    async fn bounded_channel_rejects_total_overflow() {
        let (sender, mut receiver) = channel_with_limits::<u8, u8>(2, 2);

        sender.send(1, 10).expect("first send should fit");
        sender.send(2, 20).expect("second send should fit");
        assert!(matches!(sender.send(3, 30), Err(Error::Full)));
        assert_eq!(receiver.recv().await, Some((1, 10)));
        sender.send(3, 30).expect("space freed after recv");
    }
}
