use std::sync::{Arc, LazyLock, atomic::Ordering};

use chrono::Datelike;
use parquet::{
    basic::{Compression, GzipLevel},
    data_type::{ByteArray, ByteArrayType, Int32Type, Int64Type},
    file::{properties::WriterProperties, writer::SerializedFileWriter},
    schema::{parser::parse_message_type, types::Type},
};

use super::AggregatedStats;

static SCHEMA: LazyLock<Arc<Type>> = LazyLock::new(|| {
    let schema = "
        message schema {
            REQUIRED INT32 date (DATE);
            REQUIRED BYTE_ARRAY from (UTF8);
            REQUIRED BYTE_ARRAY to (UTF8);
            REQUIRED BYTE_ARRAY started-from (UTF8);
            REQUIRED INT64 count (INTEGER(64, false));
        }
    ";

    Arc::new(parse_message_type(schema).unwrap())
});

#[expect(clippy::cast_possible_wrap)]
pub(super) fn encode_parquet(stats: &AggregatedStats) -> parquet::errors::Result<Box<[u8]>> {
    let mut buf = Vec::new();
    let mut writer = SerializedFileWriter::new(
        &mut buf,
        Arc::clone(&*SCHEMA),
        Arc::new(
            WriterProperties::builder()
                .set_compression(Compression::GZIP(GzipLevel::try_new(9).unwrap()))
                .build(),
        ),
    )?;

    let counters = stats.counters.pin();
    for ((date, from, to, started_from), count) in &counters {
        let mut row_group = writer.next_row_group()?;

        {
            let mut date_col_untyped = row_group.next_column()?.unwrap();
            let date_col = date_col_untyped.typed::<Int32Type>();
            date_col.write_batch(&[date.year_ce().1 as i32 - 1970], None, None)?;
            date_col_untyped.close()?;
        }

        {
            let mut from_col_untyped = row_group.next_column()?.unwrap();
            let from_col = from_col_untyped.typed::<ByteArrayType>();
            from_col.write_batch(&[ByteArray::from(&**from)], None, None)?;
            from_col_untyped.close()?;
        }

        {
            let mut to_col_untyped = row_group.next_column()?.unwrap();
            let to_col = to_col_untyped.typed::<ByteArrayType>();
            to_col.write_batch(&[ByteArray::from(&**to)], None, None)?;
            to_col_untyped.close()?;
        }

        {
            let mut started_from_col_untyped = row_group.next_column()?.unwrap();
            let started_from_col = started_from_col_untyped.typed::<ByteArrayType>();
            started_from_col.write_batch(&[ByteArray::from(&**started_from)], None, None)?;
            started_from_col_untyped.close()?;
        }

        {
            let mut count_col_untyped = row_group.next_column()?.unwrap();
            let count_col = count_col_untyped.typed::<Int64Type>();
            count_col.write_batch(&[count.load(Ordering::Relaxed) as i64], None, None)?;
            count_col_untyped.close()?;
        }

        row_group.close()?;
    }

    writer.close()?;

    Ok(Box::from(buf))
}
