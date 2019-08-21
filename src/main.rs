extern crate serenity;
extern crate hyper;
extern crate hyper_tls;
extern crate futures;
extern crate tokio;
extern crate serde;
extern crate serde_json;

mod client;
mod db;

use std::thread;
use std::sync::Mutex;
use serenity::Client;
use serenity::prelude::{TypeMapKey, EventHandler, Context};
use serenity::model::channel::Message;
use serenity::model::id::ChannelId;
use db::DBConnection;
use serde_json::json;
use hyper::rt;
use hyper::http::Request;
use futures::{Async, Stream, try_ready};
use futures::future::Future;
use futures::sync::mpsc;
use client::RequestState;

type HyperClient = hyper::Client<hyper_tls::HttpsConnector<hyper::client::HttpConnector>>;

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

fn send_message(client: &HyperClient, bot_token: &str, message_content: &str, channel_id: i64) -> Box<Future<Item = (), Error = ()> + Send> {
    let uri = "https://discordapp.com/api/v6/channels/".to_owned() + &channel_id.to_string() + "/messages";
    let request_body = json!({
        "content": message_content
    }).to_string();
    let request = Request::builder()
        .uri(uri)
        .method("POST")
        .header("Authorization", "Bot ".to_owned() + bot_token)
        .header("Content-Type", "application/json")
        .header("User-Agent", "JohnWickBot(https://johnwickbot.shop, 0.1)")
        .body(hyper::Body::from(request_body))
        .unwrap();

    Box::new(client.request(request)
        .and_then(|res| {
            println!("req status: {}", res.status());
            res.into_body().concat2()
        })
        .and_then(|body| {
            let s = std::str::from_utf8(&body).unwrap();
            println!("body: {}", s);
            Ok(())
        })
        .map_err(|err| {
            println!("Error sending response: {}", err);
        }))
}

struct MessageBroadcast {
    total_requests: Vec<Box<Future<Item = (), Error = ()> + Send>>,
}

impl MessageBroadcast {
    fn new(client: &HyperClient, bot_token: &str, message_content: &str) -> Self {
        /*let db = DBConnection::new().unwrap();
        let channels = db.get_channels();*/
        let channels: Vec<i64> = vec![613583885699383307];

        Self {
            total_requests: channels.into_iter().map(|v| send_message(client, bot_token, message_content, v)).collect(),
        }
    }
}

impl Future for MessageBroadcast {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        Ok(Async::Ready(()))
    }
}

struct HyperRuntime {
    client: HyperClient,
    bot_token: String,
    receiver: mpsc::UnboundedReceiver<RequestState>,
}

impl Future for HyperRuntime {
    type Item = ();

    type Error = ();

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        loop {
            let request = match try_ready!(self.receiver.poll()) {
                Some(val) => val,
                None => return Ok(Async::Ready(())),
            };

            if let RequestState::ImageBroadcast(filename) = request {
                rt::spawn(MessageBroadcast::new(&self.client, &self.bot_token, &filename));
            }
        }
    }
}

fn main() {
    let bot_token = std::env::var("DISCORD_TOKEN").unwrap();
    let mut discord_client = Client::new(bot_token.clone(), JWHandler).unwrap();

    {
        let mut data = discord_client.data.write();
        data.insert::<DBMapKey>(Mutex::new(DBConnection::new().unwrap()));
    }

    let https = hyper_tls::HttpsConnector::new(4).unwrap();
    let http_client = hyper::Client::builder().build::<_, hyper::Body>(https);

    let unb_channel = mpsc::unbounded::<RequestState>();

    let hyper_runtime = HyperRuntime {
        client: http_client,
        bot_token,
        receiver: unb_channel.1,
    };

    let client_runtime = client::ClientFuture::new(unb_channel.0).map_err(|_| println!("Client Error"));

    thread::spawn(move || {
        rt::run(client_runtime.join(hyper_runtime).and_then(|_| {
            println!("Runtimes finished");
            Ok(())
        }));
    });

    discord_client.start().unwrap();
}
