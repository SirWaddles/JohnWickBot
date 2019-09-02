extern crate postgres;

use postgres::{Connection, TlsMode, Result as DBResult};
pub use postgres::Error as DBError;

pub struct DBConnection {
    conn: Connection,
}

// Yes, I am very lazy.
pub struct DBErr;

impl From<DBError> for DBErr {
    fn from(_: DBError) -> Self {
        Self
    }
}

impl DBConnection {
    pub fn new() -> DBResult<Self> {
        Ok(Self {
            conn: Connection::connect(std::env::var("PSQL_URI").unwrap(), TlsMode::None)?,
        })
    }

    pub fn channel_exists(&self, channel_id: i64) -> Result<bool, DBErr> {
        let rows = self.conn.query("SELECT COUNT(*) FROM channels WHERE discord = $1", &[&channel_id])?;
        let row = rows.get(0);
        let val: i64 = row.get(0);
        match val {
            1 => Ok(true),
            0 => Ok(false),
            _ => Err(DBErr),
        }
    }

    pub fn get_channels(&self) -> DBResult<Vec<i64>> {
        let rows = self.conn.query("SELECT discord FROM channels", &[])?;
        Ok(rows.into_iter().map(|v| v.get(0)).collect())
    }

    pub fn insert_channel(&self, channel_id: i64) -> DBResult<()> {
        self.conn.execute("INSERT INTO channels (discord) VALUES($1) ON CONFLICT DO NOTHING", &[&channel_id])?;

        Ok(())
    }

    pub fn delete_channel(&self, channel_id: i64) -> DBResult<()> {
        self.conn.execute("DELETE FROM channels WHERE discord = $1", &[&channel_id])?;

        Ok(())
    }
}