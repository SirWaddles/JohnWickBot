extern crate postgres;

use postgres::{Connection, TlsMode, Result};

pub struct DBConnection {
    conn: Connection,
}

impl DBConnection {
    pub fn new() -> Result<Self> {
        Ok(Self {
            conn: Connection::connect(std::env::var("PSQL_URI").unwrap(), TlsMode::None)?,
        })
    }

    pub fn get_channels(&self) -> Result<Vec<i64>> {
        let rows = self.conn.query("SELECT discord FROM channels", &[])?;
        Ok(rows.into_iter().map(|v| v.get(0)).collect())
    }

    pub fn insert_channel(&self, channel_id: i64) -> Result<()> {
        self.conn.execute("INSERT INTO channels (discord) VALUES($1) ON CONFLICT DO NOTHING", &[&channel_id])?;

        Ok(())
    }

    pub fn delete_channel(&self, channel_id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM channels WHERE discord = $1", &[&channel_id])?;

        Ok(())
    }
}