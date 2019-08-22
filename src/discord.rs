use std::sync::{Arc, Mutex};
use crate::db::DBConnection;
use crate::signal::TerminationFuture;
use serenity::Client;
use serenity::client::bridge::gateway::ShardManager;
use serenity::prelude::{TypeMapKey, EventHandler, Context, Mutex as SerenityMutex};
use serenity::model::channel::Message;
use serenity::model::id::ChannelId;
use futures::{Poll, Future, Async};
use futures::future::Shared;

pub struct DiscordTermination {
    shard_manager: Arc<SerenityMutex<ShardManager>>,
    exit_status: Shared<TerminationFuture>,
}

impl DiscordTermination {
    pub fn new(shard_manager: Arc<SerenityMutex<ShardManager>>, exit_status: Shared<TerminationFuture>) -> Self {
        Self {
            shard_manager, exit_status
        }
    }
}

impl Future for DiscordTermination {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.exit_status.poll() {
            Ok(Async::Ready(_)) => {
                println!("Stopping Discord");
                let mut manager = self.shard_manager.lock();
                manager.shutdown_all();
                return Ok(Async::Ready(()));
            },
            Ok(Async::NotReady) => return Ok(Async::NotReady),
            Err(_) => return Err(()),
        }
    }
}

struct JWHandler;

impl JWHandler {
    fn send_message(&self, ctx: &Context, channel: ChannelId, message: &'static str) {
        if let Err(why) = channel.say(&ctx.http, message) {
            println!("Error sending message to channel {}: {}", channel.0, why);
        }
    }
}

impl EventHandler for JWHandler {
    fn message(&self, ctx: Context, msg: Message) {
        let data_lock = ctx.data.read();
        let db = data_lock.get::<DBMapKey>().unwrap().lock().unwrap();
        if msg.content == "!subscribe" {
            db.insert_channel(msg.channel_id.0 as i64).unwrap();
            self.send_message(&ctx, msg.channel_id, "Thanks! I'll let you know in this channel.");
        }
        if msg.content == "!unsubscribe" {
            db.delete_channel(msg.channel_id.0 as i64).unwrap();
            self.send_message(&ctx, msg.channel_id, "I'll stop sending messages here.");
        }
        if msg.content == "!help" {
            self.send_message(&ctx, msg.channel_id, "I'm a bot.");
        }
    }
}

struct DBMapKey;

impl TypeMapKey for DBMapKey {
    type Value = Mutex<DBConnection>;
}

pub fn build_client() -> Client {
    let bot_token = std::env::var("DISCORD_TOKEN").unwrap();
    let discord_client = Client::new(bot_token, JWHandler).unwrap();

    {
        let mut data = discord_client.data.write();
        data.insert::<DBMapKey>(Mutex::new(DBConnection::new().unwrap()));
    }

    discord_client
}