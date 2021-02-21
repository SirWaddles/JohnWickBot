use tokio_postgres::{NoTls, Error as DBError, Client};

#[derive(Debug)]
pub struct DBErr;

impl From<DBError> for DBErr {
    fn from(_: DBError) -> Self {
        Self
    }
}

impl std::fmt::Display for DBErr {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Database Error")
    }
}

impl std::error::Error for DBErr {}


type DBResult<T> = Result<T, DBErr>;

pub struct DBManager {
    client: Client,
}

impl DBManager {
    pub async fn new() -> DBResult<Self> {
        let conn_str = std::env::var("PSQL_URI").unwrap();
        let (client, connection) = tokio_postgres::connect(&conn_str, NoTls).await?;

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {}", e);
            }
        });
        
        Ok(Self {
            client,
        })
    }

    pub async fn channel_exists(&self, channel_id: i64) -> DBResult<bool> {
        let rows = self.client.query("SELECT COUNT(*) FROM channels WHERE discord = $1", &[&channel_id]).await?;
        let row = match rows.get(0) {
            Some(r) => r,
            None => return Err(DBErr),
        };
        let val: i64 = row.get(0);
        match val {
            1 => Ok(true),
            0 => Ok(false),
            _ => Err(DBErr),
        }
    }

    pub async fn get_channels(&self) -> DBResult<Vec<i64>> {
        let rows = self.client.query("SELECT discord FROM channels", &[]).await?;
        Ok(rows.into_iter().map(|v| v.get(0)).collect())
    }

    pub async fn insert_channel(&self, channel_id: i64) -> DBResult<()> {
        self.client.execute("INSERT INTO channels (discord) VALUES($1) ON CONFLICT DO NOTHING", &[&channel_id]).await?;

        Ok(())
    }

    pub async fn delete_channel(&self, channel_id: i64) -> DBResult<()> {
        self.client.execute("DELETE FROM channels WHERE discord = $1", &[&channel_id]).await?;

        Ok(())
    }
}