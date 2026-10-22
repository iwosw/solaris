//! Opaque block-log rows owned by mc-core's audit log (`block_log` table).
//! Keys order rows; mc-core encodes keys and values. Writes are idempotent
//! upserts plus range deletions, so a retried checkpoint never loses or
//! duplicates a row.

use redb::{ReadableTable, TableDefinition};

use super::checkpoint::NativeCheckpointReader;
use super::{NativeError, NativeStore, db};

const BLOCK_LOG: TableDefinition<&[u8], &[u8]> = TableDefinition::new("block_log");

impl NativeStore {
    /// Upsert `rows`, then delete each half-open `[start, end)` key range in
    /// `prune`, in one durable transaction.
    pub fn write_block_log(
        &self,
        rows: &[(Vec<u8>, Vec<u8>)],
        prune: &[(Vec<u8>, Vec<u8>)],
    ) -> Result<(), NativeError> {
        let mut tx = db(self.database.begin_write())?;
        db(tx.set_durability(redb::Durability::Immediate))?;
        {
            let mut table = db(tx.open_table(BLOCK_LOG))?;
            for (key, value) in rows {
                db(table.insert(key.as_slice(), value.as_slice()))?;
            }
            for (start, end) in prune {
                db(table.retain_in(start.as_slice()..end.as_slice(), |_, _| false))?;
            }
        }
        db(tx.commit())
    }
}

impl NativeCheckpointReader {
    /// Visit every block-log row in key order; a store without the table has none.
    pub fn visit_block_log(
        &self,
        mut visit: impl FnMut(&[u8], &[u8]) -> Result<(), NativeError>,
    ) -> Result<(), NativeError> {
        let table = match self.tx.open_table(BLOCK_LOG) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
            Err(error) => return Err(NativeError::Database(error.into())),
        };
        for row in db(table.iter())? {
            let (key, value) = db(row)?;
            visit(key.value(), value.value())?;
        }
        Ok(())
    }
}
