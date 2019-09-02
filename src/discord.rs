use std::sync::{Arc, Mutex};
use std::fs;
use std::fmt;
use crate::db::DBConnection;
use crate::signal::TerminationFuture;
use crate::client::MessageManager;
use serenity::Client;
use serenity::client::bridge::gateway::{ShardManager, event::ShardStageUpdateEvent};
use serenity::prelude::{TypeMapKey, EventHandler, Context, Mutex as SerenityMutex};
use serenity::gateway::ConnectionStage;
use serenity::model::{channel::Message, Permissions, id::ChannelId};
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

#[derive(Debug)]
struct JWError {
    msg: String,
}

impl JWError {
    fn new(msg: &str) -> Self {
        Self {
            msg: msg.to_owned(),
        }
    }
}

impl fmt::Display for JWError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "JW Error: {}", self.msg)
    }
}

impl From<serenity::Error> for JWError {
    fn from(err: serenity::Error) -> Self {
        Self {
            msg: err.to_string(),
        }
    }
}

impl From<crate::db::DBError> for JWError {
    fn from(err: crate::db::DBError) -> Self {
        Self {
            msg: err.to_string(),
        }
    }
}

impl From<crate::db::DBErr> for JWError {
    fn from(_: crate::db::DBErr) -> Self {
        Self::new("Database Error")
    }
}

impl From<std::io::Error> for JWError {
    fn from(err: std::io::Error) -> Self {
        Self {
            msg: err.to_string(),
        }
    }
}

impl std::error::Error for JWError {

}

type JWResult<T> = Result<T, JWError>;

struct JWHandler;

impl JWHandler {
    fn send_message(&self, ctx: &Context, channel: ChannelId, message: &str) -> JWResult<Message> {
        Ok(channel.say(&ctx.http, message)?)
    }

    fn get_permissions(&self, ctx: &Context, msg: &Message) -> JWResult<Permissions> {
        let guild_lock = match msg.guild(&ctx.cache) {
            Some(guild) => guild,
            None => return Err(JWError::new("Guild not found")),
        };
        let guild = guild_lock.read();
        let permissions = guild.member_permissions(msg.author.id.0);
        Ok(permissions)
    }

    fn subscribe_channel(&self, ctx: &Context, msg: &Message) -> JWResult<()> {
        let data_lock = ctx.data.read();
        let db = data_lock.get::<DBMapKey>().unwrap().lock().unwrap();
        let permissions = self.get_permissions(&ctx, &msg)?;
        if !permissions.contains(Permissions::MANAGE_CHANNELS) {
            self.send_message(&ctx, msg.channel_id, "You do not have the server permissions required to do this.")?;
            return Ok(());
        }
        if db.channel_exists(msg.channel_id.0 as i64)? == true {
            self.send_message(&ctx, msg.channel_id, "This channel is already subscribed.")?;
            return Ok(());
        }
        match msg.channel_id.say(&ctx.http, "Thanks! I'll let you know in this channel.") {
            Ok(_res) => db.insert_channel(msg.channel_id.0 as i64)?,
            Err(why) => {
                println!("Could not send message to channel {}: {}", msg.channel_id.0, why);
                msg.author.direct_message(ctx, |m| m.content("I was not able to subscribe to that channel. I may not have permissions to do so."))?;
            }
        };
        Ok(())
    }

    fn unsubscribe_channel(&self, ctx: &Context, msg: &Message) -> JWResult<()> {
        let data_lock = ctx.data.read();
        let db = data_lock.get::<DBMapKey>().unwrap().lock().unwrap();
        let permissions = self.get_permissions(&ctx, &msg)?;
        if !permissions.contains(Permissions::MANAGE_CHANNELS) {
            self.send_message(&ctx, msg.channel_id, "You do not have the server permissions required to do this.")?;
            return Ok(());
        }
        db.delete_channel(msg.channel_id.0 as i64)?;
        self.send_message(&ctx, msg.channel_id, "I'll stop sending messages here.")?;
        Ok(())
    }

    fn send_help_message(&self, ctx: &Context, msg: &Message) -> JWResult<()> {
        let help_string = fs::read_to_string("./helptext.txt")?;
        self.send_message(&ctx, msg.channel_id, &help_string)?;
        Ok(())
    }

    fn send_shop_message(&self, ctx: &Context, msg: &Message) -> JWResult<()> {
        let data_lock = ctx.data.read();
        let future = {
            let sender = data_lock.get::<SenderMapKey>().unwrap();
            let mut lock = sender.lock().unwrap();
            lock.add_message("request_image", "to_server".to_owned())
        };
        if let Ok(response) = future.wait() {
            let response_data = match response.get_data().as_str() {
                Some(data) => data,
                None => return Err(JWError::new("String conversion failed")),
            };
            self.send_message(&ctx, msg.channel_id, response_data)?;
        }
        Ok(())
    }

    fn handle_message(&self, ctx: &Context, msg: &Message) -> JWResult<()> {
        if msg.content == "!subscribe" {
            return self.subscribe_channel(ctx, msg);
        }
        if msg.content == "!unsubscribe" {
            return self.unsubscribe_channel(ctx, msg);
        }
        if msg.content == "!help" {
            return self.send_help_message(ctx, msg);
        }
        if msg.content == "!shop" {
            return self.send_shop_message(ctx, msg);
        }

        if msg.author.id.0 == 229419335930609664 {
            if msg.content == "!refresh" {
                let data_lock = ctx.data.read();
                let future = {
                    let sender = data_lock.get::<SenderMapKey>().unwrap();
                    let mut lock = sender.lock().unwrap();
                    lock.add_message("request_refresh", "to_server".to_owned())
                };
                if let Ok(response) = future.wait() {
                    self.send_message(&ctx, msg.channel_id, response.get_data().as_str().unwrap())?;
                }
            }

            if msg.content.len() >= 10 && &msg.content[..10] == "!broadcast" {
                let data_lock = ctx.data.read();
                let future = {
                    let sender = data_lock.get::<SenderMapKey>().unwrap();
                    let mut lock = sender.lock().unwrap();
                    lock.add_message("request_broadcast", (&msg.content[11..]).to_owned())
                };
                if let Ok(_) = future.wait() {
                    self.send_message(&ctx, msg.channel_id, "Broadcast finished")?;
                }
            }
        }

        Ok(())
    }
}

impl EventHandler for JWHandler {
    fn message(&self, ctx: Context, msg: Message) {
        match self.handle_message(&ctx, &msg) {
            Ok(_) => (),
            Err(e) => {
                println!("Error: {}", e);
            }
        };
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