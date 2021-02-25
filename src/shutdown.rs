use std::sync::{Arc, Mutex};
use std::cell::Cell;
use tokio::sync::Mutex as AsyncMutex;
use futures::channel::oneshot;
use serenity::client::bridge::gateway::ShardManager;

pub fn build_shutdown(shard_manager: &Arc<AsyncMutex<ShardManager>>) {
    let (sd_send, sd_recv) = oneshot::channel::<bool>();

    let shard_manager = Arc::clone(shard_manager);
    tokio::spawn(async move {
        let data = match sd_recv.await {
            Ok(d) => d,
            Err(_e) => return,
        };
        if data {
            let mut lock = shard_manager.lock().await;
            lock.shutdown_all().await;
        }
    });

    let send = Arc::new(Mutex::new(Cell::new(Some(sd_send))));

    ctrlc::set_handler(move || {
        let d = send.lock().unwrap().replace(None);
        match d {
            Some(sender) => {
                sender.send(true).unwrap();
            },
            None => return,
        };
    }).unwrap();
}