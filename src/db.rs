extern crate postgres;

use postgres::{Connection, TlsMode, Result};

pub struct DBConnection {
    conn: Connection,
}

impl DBConnection {
    pub fn new() -> Result<Self> {
        println!("{}", std::env::var("PSQL_URI").unwrap());
        Ok(Self {
            conn: Connection::connect(std::env::var("PSQL_URI").unwrap(), TlsMode::None)?,
        })
    }

    pub fn get_channels(&self) -> Result<Vec<i64>> {
        let rows = self.conn.query("SELECT discord FROM channels", &[])?;
        let mut channels = Vec::new();
        for row in &rows {
            channels.push(row.get(0));
        }
        Ok(channels)
    }

    pub fn insert_channel(&self, channel_id: i64) -> Result<()> {
        self.conn.execute("INSERT INTO channels (discord, channel_type) VALUES($1, $2)", &[&channel_id, &"image"])?;

        Ok(())
    }

    pub fn delete_channel(&self, channel_id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM channels WHERE discord = $1", &[&channel_id])?;

        Ok(())
    }
}