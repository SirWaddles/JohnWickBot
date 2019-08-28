use std::sync::{Arc, Mutex};
use std::fs;
use crate::db::DBConnection;
use crate::signal::TerminationFuture;
use crate::client::MessageManager;
use serenity::Client;
use serenity::client::bridge::gateway::{ShardManager, event::ShardStageUpdateEvent};
use serenity::prelude::{TypeMapKey, EventHandler, Context, Mutex as SerenityMutex};
use serenity::gateway::ConnectionStage;
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
    fn send_message(&self, ctx: &Context, channel: ChannelId, message: &str) {
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
            match db.channel_exists(msg.channel_id.0 as i64) {
                Ok(true) => {
                    self.send_message(&ctx, msg.channel_id, "This channel is already subscribed.");
                },
                Ok(false) => {
                    match msg.channel_id.say(&ctx.http, "Thanks! I'll let you know in this channel.") {
                        Ok(_res) => db.insert_channel(msg.channel_id.0 as i64).unwrap(),
                        Err(why) => {
                            println!("Could not send message to channel {}: {}", msg.channel_id.0, why);
                            if let Err(awhy) = msg.author.direct_message(&ctx, |m| m.content("I was not able to subscribe to that channel. I may not have permissions to do so.")) {
                                println!("Could not send DM to subscriber: {}", awhy);
                            }
                        }
                    };
                },
                Err(_) => {
                    println!("Database failed I guess");
                },
            };
        }
        if msg.content == "!unsubscribe" {
            db.delete_channel(msg.channel_id.0 as i64).unwrap();
            self.send_message(&ctx, msg.channel_id, "I'll stop sending messages here.");
        }
        if msg.content == "!help" {
            if let Ok(help_string) = fs::read_to_string("./helptext.txt") {
                self.send_message(&ctx, msg.channel_id, &help_string);
            }
        }
        if msg.content == "!shop" {
            let future = {
                let sender = data_lock.get::<SenderMapKey>().unwrap();
                let mut lock = sender.lock().unwrap();
                lock.add_message("request_shop", "to_server".to_owned())
            };
            let response = match future.wait() {
                Ok(val) => val,
                Err(_) => panic!("future error I guess"),
            };
            self.send_message(&ctx, msg.channel_id, response.get_data().as_str().unwrap());
        }
    }

    fn shard_stage_update(&self, ctx: Context, evt: ShardStageUpdateEvent) {
        use serenity::model::gateway::Activity;
        use serenity::model::user::OnlineStatus;

        if evt.new == ConnectionStage::Connected {
            let activity = Activity::playing("Type !help");
            let status = OnlineStatus::Online;
            ctx.set_presence(Some(activity), status);
        }
    }
}

struct DBMapKey;

impl TypeMapKey for DBMapKey {
    type Value = Mutex<DBConnection>;
}

struct SenderMapKey;

impl TypeMapKey for SenderMapKey {
    type Value = Arc<Mutex<MessageManager>>;
}

pub fn build_client(sender: Arc<Mutex<MessageManager>>) -> Client {
    let bot_token = std::env::var("DISCORD_TOKEN").unwrap();
    let discord_client = Client::new(bot_token, JWHandler).unwrap();

    {
        let mut data = discord_client.data.write();
        data.insert::<DBMapKey>(Mutex::new(DBConnection::new().unwrap()));
        data.insert::<SenderMapKey>(sender);
    }

    discord_client
}