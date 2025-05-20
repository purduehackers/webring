use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicU64, Ordering},
};

use chrono::TimeDelta;
use papaya::HashMap;
use parquet::{
    basic::{Compression, GzipLevel},
    data_type::{ByteArray, ByteArrayType, Int32Type, Int64Type},
    errors::ParquetError,
    file::{
        properties::WriterProperties,
        reader::{ChunkReader, FileReader, SerializedFileReader},
        writer::SerializedFileWriter,
    },
    record::Field,
    schema::{parser::parse_message_type, types::Type},
};
use sarlacc::Intern;

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
#[expect(clippy::cast_possible_truncation)]
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

    let mut dates = Vec::new();
    let mut froms = Vec::new();
    let mut tos = Vec::new();
    let mut started_froms = Vec::new();
    let mut counts = Vec::new();

    let epoch_date = chrono::DateTime::UNIX_EPOCH.date_naive();

    let counters = stats.counters.pin();
    for ((date, from, to, started_from), count) in &counters {
        dates.push(date.signed_duration_since(epoch_date).num_days() as i32);
        froms.push(ByteArray::from(&**from));
        tos.push(ByteArray::from(&**to));
        started_froms.push(ByteArray::from(&**started_from));
        counts.push(count.load(Ordering::Relaxed) as i64);
    }
    drop(counters);

    let mut row_group = writer.next_row_group()?;

    let mut date_col_untyped = row_group.next_column()?.unwrap();
    let date_col = date_col_untyped.typed::<Int32Type>();
    date_col.write_batch(&dates, None, None)?;
    date_col_untyped.close()?;

    let mut from_col_untyped = row_group.next_column()?.unwrap();
    let from_col = from_col_untyped.typed::<ByteArrayType>();
    from_col.write_batch(&froms, None, None)?;
    from_col_untyped.close()?;

    let mut to_col_untyped = row_group.next_column()?.unwrap();
    let to_col = to_col_untyped.typed::<ByteArrayType>();
    to_col.write_batch(&tos, None, None)?;
    to_col_untyped.close()?;

    let mut started_from_col_untyped = row_group.next_column()?.unwrap();
    let started_from_col = started_from_col_untyped.typed::<ByteArrayType>();
    started_from_col.write_batch(&started_froms, None, None)?;
    started_from_col_untyped.close()?;

    let mut count_col_untyped = row_group.next_column()?.unwrap();
    let count_col = count_col_untyped.typed::<Int64Type>();
    count_col.write_batch(&counts, None, None)?;
    count_col_untyped.close()?;

    row_group.close()?;

    writer.close()?;

    Ok(Box::from(buf))
}

pub(super) fn decode_parquet(
    file: impl ChunkReader + 'static,
) -> parquet::errors::Result<AggregatedStats> {
    if file.len() == 0 {
        return Ok(AggregatedStats {
            counters: HashMap::new(),
        });
    }

    let reader = SerializedFileReader::new(file)?;
    let meta = reader.metadata();
    let counters = HashMap::new();

    let epoch_date = chrono::DateTime::UNIX_EPOCH.date_naive();

    for i in 0..meta.num_row_groups() {
        let row_group = reader.get_row_group(i)?;

        if row_group.metadata().schema_descr().root_schema() != &**SCHEMA {
            return Err(ParquetError::General(
                "The schema of the parquet file does not match the expected schema.".to_owned(),
            ));
        }

        for maybe_row in row_group.get_row_iter(None)? {
            let row = maybe_row?;

            // The unwraps are OK because we checked everything when checking the schema
            let columns: [_; 5] = row.into_columns().try_into().unwrap();
            let columns = columns.map(|(_, field)| field);

            let date = match columns[0] {
                Field::Date(days_since_epoch) => {
                    epoch_date + TimeDelta::days(i64::from(days_since_epoch))
                }
                _ => unreachable!(),
            };
            let from = match &columns[1] {
                Field::Str(str) => Intern::<str>::from_ref(str),
                _ => unreachable!(),
            };
            let to = match &columns[2] {
                Field::Str(str) => Intern::<str>::from_ref(str),
                _ => unreachable!(),
            };
            let started_from = match &columns[3] {
                Field::Str(str) => Intern::<str>::from_ref(str),
                _ => unreachable!(),
            };
            let count = match columns[4] {
                Field::ULong(count) => AtomicU64::new(count),
                _ => unreachable!(),
            };

            counters.pin().insert((date, from, to, started_from), count);
        }
    }

    Ok(AggregatedStats { counters })
}
