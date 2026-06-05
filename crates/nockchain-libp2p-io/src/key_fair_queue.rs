use std::hash::Hash;
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{mpsc, Mutex};

pub fn channel<K: Eq + Hash, V>() -> (Sender<K, V>, Receiver<K, V>) {
    let (key_sender, key_receiver) = mpsc::unbounded_channel();
    let enqueued = Arc::new(DashMap::new());
    let sender = Sender {
        enqueued: enqueued.clone(),
        key_sender,
    };
    let receiver = Receiver {
        enqueued,
        key_receiver,
    };
    (sender, receiver)
}

pub struct Sender<K, V> {
    enqueued: Arc<DashMap<K, Mutex<V>>>,
    key_sender: mpsc::UnboundedSender<K>,
}

impl<K, V> Clone for Sender<K, V> {
    fn clone(&self) -> Self {
        Sender {
            enqueued: self.enqueued.clone(),
            key_sender: self.key_sender.clone(),
        }
    }
}

pub struct Receiver<K, V> {
    enqueued: Arc<DashMap<K, Mutex<V>>>,
    key_receiver: mpsc::UnboundedReceiver<K>,
}

pub enum Error<K> {
    SendError(mpsc::error::SendError<K>),
}

impl<K> From<mpsc::error::SendError<K>> for Error<K> {
    fn from(err: mpsc::error::SendError<K>) -> Self {
        Error::SendError(err)
    }
}

impl<K: Eq + Hash + Clone, V> Sender<K, V> {
    pub fn send(&self, key: K, value: V) -> Result<(), Error<K>> {
        let value_mutex = Mutex::new(value);
        if self.enqueued.insert(key.clone(), value_mutex).is_none() {
            Ok(self.key_sender.send(key)?)
        } else {
            Ok(())
        }
    }
}

impl<K: Eq + Hash, V> Receiver<K, V> {
    pub async fn recv(&mut self) -> Option<(K, V)> {
        let key = self.key_receiver.recv().await?;
        let opt_mutex = self.enqueued.remove(&key);
        Some(
            opt_mutex
                .map(|(k, m)| (k, m.into_inner()))
                .expect("Key from queue should be in dashmap"),
        )
    }
}
