use std::path::{Path, PathBuf};
use std::{fs, io};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vma {
    pub start: usize,
    pub end: usize,
    pub perms: String,
    pub path: PathBuf,
}

impl Vma {
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_shared(&self) -> bool {
        self.perms.as_bytes().get(3) == Some(&b's')
    }
}

pub fn read_pma_vmas(work_dir: &Path) -> io::Result<Vec<Vma>> {
    let replay_dir = work_dir.join("replay-pma");
    let replay_dir = fs::canonicalize(&replay_dir).unwrap_or(replay_dir);
    let maps = fs::read_to_string("/proc/self/maps")?;
    parse_proc_maps(&maps, &replay_dir)
}

pub fn parse_proc_maps(contents: &str, replay_dir: &Path) -> io::Result<Vec<Vma>> {
    let mut out = Vec::new();
    for line in contents.lines() {
        let Some(parsed) = parse_proc_maps_line(line)? else {
            continue;
        };
        let Some(path) = parsed.path else {
            continue;
        };
        if !path.starts_with(replay_dir) {
            continue;
        }

        out.push(Vma {
            start: parsed.start,
            end: parsed.end,
            perms: parsed.perms,
            path,
        });
    }
    Ok(out)
}

pub fn read_nockstack_vmas() -> io::Result<Vec<Vma>> {
    let maps = fs::read_to_string("/proc/self/maps")?;
    select_nockstack_vmas_from_maps(
        &maps,
        nockapp::utils::NOCK_STACK_SIZE_MEDIUM * 8,
        NOCKSTACK_SIZE_TOLERANCE,
    )
}

const NOCKSTACK_SIZE_TOLERANCE: f64 = 0.05;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcMapEntry {
    start: usize,
    end: usize,
    perms: String,
    path: Option<PathBuf>,
}

fn parse_proc_maps_line(line: &str) -> io::Result<Option<ProcMapEntry>> {
    let mut parts = line.split_whitespace();
    let Some(range) = parts.next() else {
        return Ok(None);
    };
    let Some(perms) = parts.next() else {
        return Ok(None);
    };
    let _offset = parts.next();
    let _dev = parts.next();
    let _inode = parts.next();
    let path = parts.next().map(PathBuf::from);

    let (start_s, end_s) = range
        .split_once('-')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("bad range: {range}")))?;
    let start = usize::from_str_radix(start_s, 16)
        .map_err(|source| io::Error::new(io::ErrorKind::InvalidData, source))?;
    let end = usize::from_str_radix(end_s, 16)
        .map_err(|source| io::Error::new(io::ErrorKind::InvalidData, source))?;

    Ok(Some(ProcMapEntry {
        start,
        end,
        perms: perms.to_string(),
        path,
    }))
}

pub fn select_nockstack_vmas_from_maps(
    contents: &str,
    expected_size_bytes: usize,
    tolerance_fraction: f64,
) -> io::Result<Vec<Vma>> {
    let tolerance = (expected_size_bytes as f64 * tolerance_fraction) as usize;
    let min_size = expected_size_bytes.saturating_sub(tolerance);
    let max_size = expected_size_bytes.saturating_add(tolerance);
    let mut matches = Vec::new();

    for line in contents.lines() {
        let Some(parsed) = parse_proc_maps_line(line)? else {
            continue;
        };
        if parsed.path.is_some() {
            continue;
        }
        if parsed.perms != "rw-p" {
            continue;
        }

        let size = parsed.end.saturating_sub(parsed.start);
        if size < min_size || size > max_size {
            continue;
        }

        matches.push(Vma {
            start: parsed.start,
            end: parsed.end,
            perms: parsed.perms,
            path: PathBuf::from("[anon:nockstack]"),
        });
    }

    if matches.len() > 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "ambiguous NockStack VMA detection: {} anonymous rw-p mappings match expected size {} bytes",
                matches.len(),
                expected_size_bytes
            ),
        ));
    }

    Ok(matches)
}

pub fn page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

pub fn reduce_mincore_bitmap(bitmap: &[u8]) -> (usize, usize) {
    let resident = bitmap.iter().filter(|byte| (**byte & 1) == 1).count();
    (resident, bitmap.len())
}

pub fn resident_pages(vma: &Vma) -> io::Result<(usize, usize)> {
    let ps = page_size();
    let total_pages = vma.len() / ps;
    if total_pages == 0 {
        return Ok((0, 0));
    }

    let mut bitmap = vec![0u8; total_pages];
    let ret = unsafe {
        libc::mincore(
            vma.start as *mut libc::c_void,
            vma.len(),
            bitmap.as_mut_ptr() as *mut _,
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(reduce_mincore_bitmap(&bitmap))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_replay_pma_vmas() {
        let replay_dir = PathBuf::from("/tmp/work/replay-pma");
        let maps = "\
7f0a0000-7f0a1000 rw-s 00000000 00:00 0 /tmp/work/replay-pma/slab-0.bin\n\
7f0a1000-7f0a2000 rw-p 00000000 00:00 0 /tmp/work/replay-pma/slab-1.bin\n\
7f0a2000-7f0a3000 rw-s 00000000 00:00 0 /tmp/work/elsewhere.bin\n";

        let parsed = parse_proc_maps(maps, &replay_dir).expect("parse maps");

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].start, 0x7f0a0000);
        assert_eq!(parsed[0].end, 0x7f0a1000);
        assert!(parsed[0].is_shared());
        assert!(!parsed[1].is_shared());
    }

    #[test]
    fn selects_anonymous_medium_sized_nockstack_vma() {
        let expected = nockapp::utils::NOCK_STACK_SIZE_MEDIUM * 8;
        let start = 0x7f0000000000usize;
        let end = start + expected;
        let maps = format!(
            "\
555555554000-555555575000 r--p 00000000 08:02 1 /bin/nockchain-bench\n\
600000000000-600000001000 rw-p 00000000 00:00 0 [heap]\n\
{start:x}-{end:x} rw-p 00000000 00:00 0\n\
7ffc00000000-7ffc00021000 rw-p 00000000 00:00 0 [stack]\n"
        );

        let parsed = select_nockstack_vmas_from_maps(&maps, expected, 0.05).expect("nockstack vma");

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].start, start);
        assert_eq!(parsed[0].end, end);
        assert_eq!(parsed[0].perms, "rw-p");
        assert_eq!(parsed[0].path, PathBuf::from("[anon:nockstack]"));
    }

    #[test]
    fn nockstack_selector_rejects_largest_anonymous_fallback_outside_tolerance() {
        let expected = nockapp::utils::NOCK_STACK_SIZE_MEDIUM * 8;
        let too_small = expected / 2;
        let start = 0x7f1000000000usize;
        let end = start + too_small;
        let maps = format!(
            "\
{start:x}-{end:x} rw-p 00000000 00:00 0\n\
7f2000000000-7f2001000000 rw-p 00000000 00:00 0\n"
        );

        let parsed = select_nockstack_vmas_from_maps(&maps, expected, 0.05)
            .expect("strict selector should parse maps");

        assert!(parsed.is_empty());
    }

    #[test]
    fn mincore_bitmap_reduction_counts_only_low_bit() {
        let (resident, total) = reduce_mincore_bitmap(&[0, 1, 2, 3, 4, 5]);
        assert_eq!(resident, 3);
        assert_eq!(total, 6);
    }
}
