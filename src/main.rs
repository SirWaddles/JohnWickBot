use std::env;
use std::sync::{Arc, Mutex as SMutex};
use std::error::Error;
use chrono::prelude::*;
use serenity::{
    async_trait,
    model::{channel::Message, gateway::Ready, permissions::Permissions, id::ChannelId},
    prelude::*,
};

mod db;
mod broadcast;
mod shutdown;
mod client;

type BoxedError = Box<dyn Error + Send + Sync>;
type JWResult<T> = Result<T, BoxedError>;

struct BotToken {}
impl TypeMapKey for BotToken {
    type Value = String;
}

struct HttpClient {}
impl TypeMapKey for HttpClient {
    type Value = Arc<hyper::Client<hyper_tls::HttpsConnector<hyper::client::HttpConnector>>>;
}

struct DBManager {}
impl TypeMapKey for DBManager {
    type Value = Arc<db::DBManager>;
}

impl TypeMapKey for client::ClientManager {
    type Value = Arc<SMutex<client::ClientManager>>;
}

struct Handler;

impl Handler {
    async fn get_permissions_user(&self, ctx: &Context, msg: &Message) -> Result<Permissions, BoxedError> {
        let guild_id = match msg.guild_id {
            Some(g) => g,
            None => return Err("No Guild ID Present on Message".into()),
        };

        let guild = match ctx.cache.guild(guild_id).await {
            Some(g) => g,
            None => return Err("No Guild in Cache".into()),
        };

        let permissions = guild.member_permissions(ctx, msg.author.id).await?;

        Ok(permissions)
    }

    async fn send_message(&self, ctx: &Context, channel: ChannelId, message: &str) -> JWResult<Message> {
        Ok(channel.say(&ctx.http, message).await?)
    }

    async fn subscribe_channel(&self, ctx: &Context, msg: &Message) -> JWResult<()> {
        let permissions = self.get_permissions_user(&ctx, &msg).await?;
        if !permissions.contains(Permissions::MANAGE_CHANNELS) {
            self.send_message(&ctx, msg.channel_id, "You do not have the server permissions required to do this.").await?;
            return Ok(());
        }

        let db = {
            let lock = ctx.data.read().await;
            Arc::clone(lock.get::<DBManager>().unwrap())
        };

        if db.channel_exists(msg.channel_id.0 as i64).await? == true {
            self.send_message(&ctx, msg.channel_id, "This channel is already subscribed.").await?;
            return Ok(());
        }
        match msg.channel_id.say(&ctx.http, "Thanks! I'll let you know in this channel.").await {
            Ok(_res) => db.insert_channel(msg.channel_id.0 as i64).await?,
            Err(why) => {
                println!("Could not send message to channel {}: {}", msg.channel_id.0, why);
                msg.author.direct_message(ctx, |m| m.content("I was not able to subscribe to that channel. I may not have permissions to do so.")).await?;
            }
        };
        Ok(())
    }

    async fn unsubscribe_channel(&self, ctx: &Context, msg: &Message) -> JWResult<()> {
        let db = {
            let lock = ctx.data.read().await;
            Arc::clone(lock.get::<DBManager>().unwrap())
        };

        let permissions = self.get_permissions_user(&ctx, &msg).await?;
        if !permissions.contains(Permissions::MANAGE_CHANNELS) {
            self.send_message(&ctx, msg.channel_id, "You do not have the server permissions required to do this.").await?;
            return Ok(());
        }

        db.delete_channel(msg.channel_id.0 as i64).await?;
        self.send_message(&ctx, msg.channel_id, "I'll stop sending messages here.").await?;
        Ok(())
    }

    async fn request_refresh(&self, ctx: &Context, msg: &Message) -> JWResult<()> {
        let manager = {
            let lock = ctx.data.read().await;
            Arc::clone(lock.get::<client::ClientManager>().unwrap())
        };

        let req = client::send_message(&manager, "request_refresh", "to_server".to_owned()).await?;
        let url = match req.get_data().as_str() {
            Some(d) => d,
            None => {
                return Err("Value not a String".into());
            }
        };
        self.send_message(&ctx, msg.channel_id, url).await?;

        Ok(())
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        println!("{} {} is connected!", ready.user.name, ctx.shard_id);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.content == "!shop" {

            let utc: DateTime<Utc> = Utc::now(); 
            let reply = "https://wickshopbot.com/".to_owned() + &utc.year().to_string() + "_" + &utc.month0().to_string() + "_" + &utc.day().to_string() + ".png";

            if let Err(why) = msg.channel_id.say(&ctx.http, &reply).await {
                println!("Error sending message: {:?}", why);
            }
        }

        if msg.content == "!subscribe" {
            match self.subscribe_channel(&ctx, &msg).await {
                Ok(_) => return,
                Err(e) => {
                    println!("Error: {}", e);
                    return;
                }
            };
        }

        if msg.content == "!unsubscribe" {
            match self.unsubscribe_channel(&ctx, &msg).await {
                Ok(_) => return,
                Err(e) => {
                    println!("Error: {}", e);
                    return;
                }
            };
        }

        if msg.author.id.0 == 229419335930609664 {
            if msg.content == "!refresh" {
                match self.request_refresh(&ctx, &msg).await {
                    Ok(_) => (),
                    Err(e) => {
                        println!("Error: {}", e);
                    },
                };
                return;
            }


            if msg.content.len() >= 10 && &msg.content[..10] == "!broadcast" {
                broadcast_message(ctx.data, msg.content[11..].to_owned());
            }
        }
    }
}

fn broadcast_message(context: Arc<RwLock<TypeMap>>, message: String) {
    tokio::spawn(async move { 
        let (token, http, db) = {
            let data_lock = context.read().await;
            let token = data_lock.get::<BotToken>().unwrap().clone();
            let http = data_lock.get::<HttpClient>().unwrap();
            let db = data_lock.get::<DBManager>().unwrap();
            (token, Arc::clone(http), Arc::clone(db))
        };
        let channels = match db.get_channels().await {
            Ok(r) => r,
            Err(e) => {
                println!("DB Error: {:#?}", e);
                return;
            },
        };

        broadcast::MessageBroadcast::new(db, channels, http, token, &message).await;
    });
}

#[tokio::main]
async fn main() {
    let token = env::var("DISCORD_TOKEN").expect("token");
    let mut client = Client::builder(&token)
        .event_handler(Handler)
        .await
        .expect("Error creating client");

    let https = hyper_tls::HttpsConnector::new();
    let http_client = hyper::Client::builder().build::<_, hyper::Body>(https);
    println!("Connecting to Database");
    let db_man = db::DBManager::new().await.unwrap();

    println!("Connecting to JohnWick Server");
    let client_man = client::connect_client();

    {
        let mut lock = client_man.lock().unwrap();
        let client_data = Arc::clone(&client.data);
        lock.set_broadcast_hook("image", move |v| {
            if let Some(val) = v.as_str() {
                let msg = "https://wickshopbot.com/".to_owned() + val; 
                broadcast_message(Arc::clone(&client_data), msg);
            }
        });
    }

    {
        println!("Writing Context");
        let mut data = client.data.write().await;
        data.insert::<BotToken>(token);
        data.insert::<HttpClient>(Arc::new(http_client));
        data.insert::<DBManager>(Arc::new(db_man));
        data.insert::<client::ClientManager>(client_man);
    }

    shutdown::build_shutdown(&client.shard_manager);

    println!("Starting Bot");
    if let Err(why) = client.start_autosharded().await {
        println!("An error occurred while running the client: {:?}", why);
    }
}