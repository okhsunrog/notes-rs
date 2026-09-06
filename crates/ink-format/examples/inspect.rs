//! Inspect a chunk without any application or database dependency.
use ink_format::{Result, chunk::Chunk};
fn main() -> Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: inspect <file.inkchnk>")?;
    let chunk = Chunk::read_from(std::fs::File::open(path)?)?;
    println!(
        "{} samples, {} segments, {} columns",
        chunk.count()?,
        chunk.segments.len(),
        chunk.columns.len()
    );
    for column in chunk.columns {
        println!(
            "axis {}: {:?}, {} valid bytes",
            column.semantic,
            column.dtype,
            column.values.len()
        );
    }
    Ok(())
}
