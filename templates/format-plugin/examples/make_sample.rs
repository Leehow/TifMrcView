// Shared with the host tests; no dependencies or image data downloads.
pub fn sample() -> Vec<u8> {
    let mut bytes = b"R16\0".to_vec();
    for n in [4u32, 3, 2] {
        bytes.extend_from_slice(&n.to_le_bytes());
    }
    for i in 0..24u16 {
        bytes.extend_from_slice(&(i * 1000).to_le_bytes());
    }
    bytes
}

fn main() -> std::io::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "sample.r16".into());
    std::fs::write(&path, sample())?;
    println!("{path}: 4 × 3 × 2, u16 (64 bytes)");
    Ok(())
}
