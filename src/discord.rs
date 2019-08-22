use std::sync::Mutex;
use crate::db::DBConnection;
use serenity::Client;
use serenity::prelude::{TypeMapKey, EventHandler, Context};
use serenity::model::channel::Message;
use serenity::model::id::ChannelId;

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