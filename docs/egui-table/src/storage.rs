use anyhow::{Context, Result, ensure};
use egui_atlas_table::{ColumnBlock, DataSource, Schema};
use prost::Message;
use rayon::prelude::*;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

pub const GROUP_ROWS: u32 = 16_384;
pub const COLUMNS: [&str; 30] = [
    "Record ID",
    "Name",
    "Company",
    "Country",
    "Status",
    "Revenue",
    "Email",
    "City",
    "Department",
    "Role",
    "Plan",
    "Industry",
    "Region",
    "Currency",
    "Created",
    "Last active",
    "Seats",
    "Orders",
    "Score",
    "Growth",
    "Owner",
    "Channel",
    "Segment",
    "Priority",
    "Language",
    "Timezone",
    "Renewal",
    "Risk",
    "Tags",
    "Notes",
];
pub const GROUPS: [(&str, std::ops::Range<usize>); 5] = [
    ("Overview", 0..6),
    ("People & organization", 6..12),
    ("Location & activity", 12..18),
    ("Performance & sales", 18..24),
    ("Preferences & context", 24..30),
];
const MAGIC: &[u8; 8] = b"ATLASPB1";
const END: &[u8; 8] = b"ATLASEND";
const MAX_BLOCK: u32 = 32 * 1024 * 1024;

#[derive(Clone, PartialEq, Message)]
struct ProtoColumnBlock {
    #[prost(string, repeated, tag = "1")]
    pub dictionary: Vec<String>,
    #[prost(uint32, repeated, packed = "true", tag = "2")]
    pub codes: Vec<u32>,
}
#[derive(Clone, PartialEq, Message)]
pub struct BlockLocation {
    #[prost(uint64, tag = "1")]
    pub offset: u64,
    #[prost(uint32, tag = "2")]
    pub compressed_len: u32,
    #[prost(uint32, tag = "3")]
    pub raw_len: u32,
}
#[derive(Clone, PartialEq, Message)]
pub struct Index {
    #[prost(uint32, tag = "1")]
    pub version: u32,
    #[prost(uint32, tag = "2")]
    pub rows: u32,
    #[prost(uint32, tag = "3")]
    pub group_rows: u32,
    #[prost(string, repeated, tag = "4")]
    pub columns: Vec<String>,
    #[prost(message, repeated, tag = "5")]
    pub blocks: Vec<BlockLocation>,
}

pub struct Store {
    file: File,
    pub index: Index,
    pub file_bytes: u64,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path).with_context(|| format!("Opening {}", path.display()))?;
        let len = file.metadata()?.len();
        ensure!(len >= 24, "File is truncated");
        let mut magic = [0; 8];
        file.read_exact(&mut magic)?;
        ensure!(&magic == MAGIC, "Not an Atlas protobuf/zstd file");
        file.seek(SeekFrom::End(-16))?;
        let mut trailer = [0; 16];
        file.read_exact(&mut trailer)?;
        ensure!(
            &trailer[8..] == END,
            "Incomplete file: index footer is missing"
        );
        let size = u64::from_le_bytes(trailer[..8].try_into()?);
        ensure!(
            size <= 64 * 1024 * 1024 && size <= len - 24,
            "Invalid index size"
        );
        let index_start = len - 16 - size;
        file.seek(SeekFrom::Start(index_start))?;
        let mut compressed = vec![0; size as usize];
        file.read_exact(&mut compressed)?;
        let raw = zstd::bulk::decompress(&compressed, 64 * 1024 * 1024)?;
        let index = Index::decode(raw.as_slice()).context("Decoding protobuf index")?;
        ensure!(
            index.version == 1,
            "Unsupported file version {}",
            index.version
        );
        ensure!(
            index.rows > 0 && index.rows <= 100_000_000,
            "Unsupported row count"
        );
        ensure!(
            index.group_rows > 0 && index.group_rows <= 65_536,
            "Invalid group size"
        );
        ensure!(index.columns.len() == 30, "Expected 30 columns");
        let count = index.rows.div_ceil(index.group_rows) as usize * index.columns.len();
        ensure!(index.blocks.len() == count, "Incomplete block index");
        let mut previous = 8;
        for block in &index.blocks {
            ensure!(
                block.offset == previous
                    && block.compressed_len > 0
                    && block.compressed_len <= MAX_BLOCK
                    && block.raw_len > 0
                    && block.raw_len <= MAX_BLOCK,
                "Invalid block bounds"
            );
            previous = block
                .offset
                .checked_add(block.compressed_len as u64)
                .context("Block offset overflow")?;
            ensure!(previous <= index_start, "Block extends beyond data");
        }
        ensure!(previous == index_start, "Unexpected data before index");
        Ok(Self {
            file,
            index,
            file_bytes: len,
        })
    }
    pub fn groups(&self) -> u32 {
        self.index.rows.div_ceil(self.index.group_rows)
    }
    pub fn block_len(&self, group: u32) -> u32 {
        self.index
            .group_rows
            .min(self.index.rows - group * self.index.group_rows)
    }
    pub fn read_block(&self, group: u32, column: usize) -> Result<ColumnBlock> {
        ensure!(group < self.groups() && column < 30, "Block out of range");
        let location = &self.index.blocks[group as usize * 30 + column];
        let mut compressed = vec![0; location.compressed_len as usize];
        read_at(&self.file, &mut compressed, location.offset)?;
        let raw = zstd::bulk::decompress(&compressed, location.raw_len as usize)
            .context("Decompressing column block")?;
        ensure!(
            raw.len() == location.raw_len as usize,
            "Wrong decoded block length"
        );
        let block = ProtoColumnBlock::decode(raw.as_slice()).context("Decoding protobuf column")?;
        ensure!(
            block.codes.len() == self.block_len(group) as usize,
            "Wrong column row count"
        );
        ColumnBlock::new(block.dictionary, block.codes)
    }
}
#[cfg(unix)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
}
#[cfg(windows)]
fn read_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_read(buf, offset)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        offset += n as u64;
        buf = &mut buf[n..];
    }
    Ok(())
}
fn compress(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), 3)?;
    encoder.include_checksum(true)?;
    encoder.write_all(bytes)?;
    Ok(encoder.finish()?)
}

pub fn generate(path: &Path, rows: u32, progress: impl Fn(u32, u32)) -> Result<()> {
    ensure!(
        rows > 0 && rows <= 100_000_000,
        "Rows must be between 1 and 100,000,000"
    );
    ensure!(
        !path.exists(),
        "{} already exists; choose a new path",
        path.display()
    );
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let partial = path.with_extension("zst.partial");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .with_context(|| {
            format!(
                "Creating {} (remove an abandoned .partial file to retry)",
                partial.display()
            )
        })?;
    let result = (|| -> Result<()> {
        file.write_all(MAGIC)?;
        let mut index = Index {
            version: 1,
            rows,
            group_rows: GROUP_ROWS,
            columns: COLUMNS.iter().map(|s| s.to_string()).collect(),
            blocks: Vec::new(),
        };
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(worker_count())
            .build()?;
        let groups = rows.div_ceil(GROUP_ROWS);
        let mut position = 8;
        for group in 0..groups {
            let start = group * GROUP_ROWS;
            let n = GROUP_ROWS.min(rows - start);
            let columns: Vec<Result<(Vec<u8>, u32)>> = pool.install(|| {
                (0..30)
                    .into_par_iter()
                    .map(|column| {
                        let block = sample_block(start, n, column);
                        let (dictionary, codes) = block.into_parts();
                        let raw = ProtoColumnBlock { dictionary, codes }.encode_to_vec();
                        Ok((compress(&raw)?, raw.len() as u32))
                    })
                    .collect()
            });
            for column in columns {
                let (compressed, raw_len) = column?;
                index.blocks.push(BlockLocation {
                    offset: position,
                    compressed_len: compressed.len() as u32,
                    raw_len,
                });
                file.write_all(&compressed)?;
                position += compressed.len() as u64;
            }
            progress(group + 1, groups);
        }
        let footer = compress(&index.encode_to_vec())?;
        file.write_all(&footer)?;
        file.write_all(&(footer.len() as u64).to_le_bytes())?;
        file.write_all(END)?;
        file.sync_all()?;
        // Hard-link publication is atomic and will not replace an existing dataset.
        std::fs::hard_link(&partial, path).context("Publishing completed dataset")?;
        Ok(())
    })();
    drop(file);
    let cleanup = std::fs::remove_file(&partial);
    result?;
    cleanup?;
    Ok(())
}
pub fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(2).clamp(1, 6))
        .unwrap_or(2)
}
fn hash(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}
fn dictionary(column: usize) -> Vec<String> {
    const FIRST: &[&str] = &[
        "Amelia",
        "Noah",
        "Olivia",
        "Liam",
        "Charlotte",
        "James",
        "Sofia",
        "Lucas",
        "Mia",
        "Ethan",
        "Isabella",
        "Oliver",
        "Chloé",
        "Leo",
        "Elena",
        "Arjun",
        "Yuki",
        "Maya",
        "Oscar",
        "Zoe",
        "Daniel",
        "Alice",
        "Ava",
        "Felix",
        "Sara",
        "Hugo",
        "Emma",
        "Theo",
        "Nora",
        "Kai",
        "Inès",
        "Mateo",
    ];
    const LAST: &[&str] = &[
        "Morgan", "Chen", "Patel", "Rivera", "Kim", "Anderson", "Silva", "Martin", "Wilson",
        "Garcia", "Brown", "Taylor", "Sato", "Müller", "Dubois", "Singh", "Lee", "Clark", "Lewis",
        "Walker", "Hall", "Young", "Allen", "King", "Wright", "Scott", "Green", "Baker", "Adams",
        "Hill", "Nelson", "Wong",
    ];
    match column {
        1 | 6 | 20 => FIRST
            .iter()
            .flat_map(|f| {
                LAST.iter().map(move |l| {
                    if column == 6 {
                        format!("{}.{}@example.com", f.to_lowercase(), l.to_lowercase())
                    } else {
                        format!("{f} {l}")
                    }
                })
            })
            .collect(),
        2 => strings(&[
            "Acme Labs",
            "Northstar",
            "Linear Systems",
            "Meridian",
            "Cobalt Works",
            "Fable Studio",
            "Orbit Digital",
            "Juniper",
            "Aurora Health",
            "Atlas Energy",
            "Vertex",
            "Nimbus",
            "Cedar Finance",
            "Stripe Research",
            "Lumen",
            "Arcadia",
        ]),
        3 => strings(&[
            "United States",
            "United Kingdom",
            "Germany",
            "France",
            "Canada",
            "Japan",
            "Australia",
            "India",
            "Brazil",
            "Sweden",
            "Singapore",
            "Netherlands",
        ]),
        4 => strings(&["Active", "Active", "Active", "Trial", "Paused", "Archived"]),
        5 => (0..4096)
            .map(|n| format!("${}.{:02}", 250 + n * 137, n % 100))
            .collect(),
        7 => strings(&[
            "New York",
            "London",
            "Berlin",
            "Paris",
            "Toronto",
            "Tokyo",
            "Sydney",
            "Mumbai",
            "São Paulo",
            "Stockholm",
            "Singapore",
            "Amsterdam",
        ]),
        8 => strings(&[
            "Engineering",
            "Design",
            "Sales",
            "Marketing",
            "Operations",
            "Finance",
            "Research",
            "Customer success",
        ]),
        9 => strings(&[
            "Director",
            "Manager",
            "Lead",
            "Analyst",
            "Engineer",
            "Designer",
            "Specialist",
            "Founder",
        ]),
        10 => strings(&["Enterprise", "Business", "Professional", "Starter", "Free"]),
        11 => strings(&[
            "Technology",
            "Healthcare",
            "Finance",
            "Education",
            "Manufacturing",
            "Retail",
            "Energy",
            "Media",
        ]),
        12 => strings(&[
            "Americas",
            "Europe",
            "Asia Pacific",
            "Middle East",
            "Africa",
        ]),
        13 => strings(&["USD", "EUR", "GBP", "JPY", "CAD", "AUD", "INR", "BRL"]),
        14 | 15 | 26 => (0..336)
            .map(|n| format!("2026-{:02}-{:02}", n / 28 + 1, n % 28 + 1))
            .collect(),
        16 | 17 => (1..=2048).map(|n| n.to_string()).collect(),
        18 => (0..=100).map(|n| format!("{n}/100")).collect(),
        19 => (0..400)
            .map(|n| format!("{:+.1}%", n as f64 / 10.0 - 10.0))
            .collect(),
        21 => strings(&[
            "Organic", "Referral", "Partner", "Direct", "Campaign", "Event",
        ]),
        22 => strings(&["Strategic", "Mid-market", "Small business", "Startup"]),
        23 => strings(&["Normal", "High", "Urgent", "Low"]),
        24 => strings(&[
            "English",
            "French",
            "German",
            "Japanese",
            "Spanish",
            "Portuguese",
        ]),
        25 => strings(&[
            "UTC-08:00",
            "UTC-05:00",
            "UTC+00:00",
            "UTC+01:00",
            "UTC+05:30",
            "UTC+09:00",
        ]),
        27 => strings(&["Low", "Low", "Medium", "High", "Review"]),
        28 => strings(&[
            "Growth, priority",
            "New customer",
            "Renewal due",
            "Expansion",
            "Partner account",
            "Self-service",
            "Pilot program",
            "Long-term",
        ]),
        29 => strings(&[
            "Quarterly review scheduled",
            "Interested in expansion",
            "Onboarding complete",
            "Awaiting procurement",
            "Strong product adoption",
            "Follow up next week",
            "Annual agreement",
            "Technical evaluation",
        ]),
        _ => unreachable!(),
    }
}
pub fn sample_block(start: u32, n: u32, column: usize) -> ColumnBlock {
    if column == 0 {
        return ColumnBlock::new(
            (start..start + n)
                .map(|id| format!("AT-{:08}", id + 1))
                .collect(),
            (0..n).collect(),
        )
        .expect("valid generated block");
    }
    let dictionary = dictionary(column);
    let codes = (start..start + n)
        .map(|row| {
            // Country/city and name/email deliberately correlate; other columns vary independently.
            let salt = match column {
                7 => 3,
                6 => 1,
                _ => column,
            } as u64;
            (hash(row as u64 ^ salt.wrapping_mul(0x9e3779b97f4a7c15)) % dictionary.len() as u64)
                as u32
        })
        .collect();
    ColumnBlock::new(dictionary, codes).expect("valid generated block")
}

impl DataSource for Store {
    fn schema(&self) -> Schema {
        Schema {
            rows: self.index.rows,
            group_rows: self.index.group_rows,
            columns: self.index.columns.clone(),
        }
    }
    fn read_block(&self, group: u32, column: usize) -> Result<ColumnBlock> {
        Store::read_block(self, group, column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_random_access_and_corruption() -> Result<()> {
        let path = std::env::temp_dir().join(format!("atlas-store-{}.pb.zst", std::process::id()));
        let _ = std::fs::remove_file(&path);
        generate(&path, GROUP_ROWS + 7, |_, _| {})?;
        let store = Store::open(&path)?;
        for (g, c) in [(1, 29), (0, 0), (1, 0), (0, 3)] {
            assert_eq!(
                store.read_block(g, c)?,
                sample_block(g * GROUP_ROWS, store.block_len(g), c)
            );
        }
        assert_eq!(store.read_block(1, 0)?.value(6), "AT-00016391");
        assert!(generate(&path, 10, |_, _| {}).is_err());
        assert!(store.read_block(2, 0).is_err());
        let first = &store.index.blocks[0];
        let checksum_offset = first.offset + first.compressed_len as u64 - 1;
        let mut original = [0];
        read_at(&store.file, &mut original, checksum_offset)?;
        let mut writer = OpenOptions::new().write(true).open(&path)?;
        writer.seek(SeekFrom::Start(checksum_offset))?;
        writer.write_all(&[original[0] ^ 0xff])?;
        drop(writer);
        assert!(
            store.read_block(0, 0).is_err(),
            "zstd checksum must detect corruption"
        );
        drop(store);
        let f = OpenOptions::new().write(true).open(&path)?;
        f.set_len(32)?;
        drop(f);
        assert!(Store::open(&path).is_err());
        std::fs::remove_file(path)?;
        Ok(())
    }
}
