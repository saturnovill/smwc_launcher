//! Aplicación de parches BPS e IPS.

pub fn apply(patch: &[u8], rom: &[u8]) -> Result<Vec<u8>, String> {
    if patch.starts_with(b"BPS1") {
        bps(patch, rom)
    } else if patch.starts_with(b"PATCH") {
        ips(patch, rom)
    } else {
        Err(crate::t("Formato de parche desconocido", "Unknown patch format").into())
    }
}

fn be(b: &[u8]) -> usize {
    b.iter().fold(0, |a, &x| a << 8 | x as usize)
}

fn ips(p: &[u8], rom: &[u8]) -> Result<Vec<u8>, String> {
    let get = |i: usize, n: usize| p.get(i..i + n).ok_or(crate::t("Parche IPS truncado", "Truncated IPS patch").to_string());
    let mut out = rom.to_vec();
    let mut i = 5;
    loop {
        let off = get(i, 3)?;
        if off == b"EOF" {
            if let Some(t) = p.get(i + 3..i + 6) {
                out.truncate(be(t));
            }
            return Ok(out);
        }
        let (off, size) = (be(off), be(get(i + 3, 2)?));
        i += 5;
        let data = if size == 0 {
            let rle = get(i, 3)?;
            i += 3;
            vec![rle[2]; be(&rle[..2])]
        } else {
            i += size;
            get(i - size, size)?.to_vec()
        };
        if out.len() < off + data.len() {
            out.resize(off + data.len(), 0);
        }
        out[off..off + data.len()].copy_from_slice(&data);
    }
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn num(&mut self) -> Result<u64, String> {
        let (mut data, mut shift) = (0u64, 1u64);
        loop {
            let x = *self.b.get(self.i).ok_or(crate::t("Parche BPS truncado", "Truncated BPS patch"))? as u64;
            self.i += 1;
            data = data.wrapping_add((x & 0x7f).wrapping_mul(shift));
            if x & 0x80 != 0 {
                return Ok(data);
            }
            shift <<= 7;
            data = data.wrapping_add(shift);
        }
    }
    fn bytes(&mut self, n: usize) -> Result<&[u8], String> {
        self.i += n;
        self.b.get(self.i - n..self.i).ok_or(crate::t("Parche BPS truncado", "Truncated BPS patch").into())
    }
}

fn bps(p: &[u8], src: &[u8]) -> Result<Vec<u8>, String> {
    let bad = || crate::t("Parche BPS corrupto", "Corrupt BPS patch").to_string();
    if p.len() < 16 {
        return Err(bad());
    }
    let crc = |at: usize| u32::from_le_bytes(p[at..at + 4].try_into().unwrap());
    let end = p.len() - 12;
    if crc32fast::hash(&p[..p.len() - 4]) != crc(p.len() - 4) {
        return Err(bad());
    }
    if crc32fast::hash(src) != crc(end) {
        return Err(crate::t("La ROM base no es la que espera el parche (usa una ROM limpia de SMW USA)", "The base ROM is not the one the patch expects (use a clean SMW USA ROM)").into());
    }
    let mut r = Reader { b: &p[..end], i: 4 };
    r.num()?; // tamaño de origen, ya validado por CRC
    let size = r.num()? as usize;
    let meta = r.num()? as usize;
    r.bytes(meta)?;
    let mut out = Vec::with_capacity(size);
    let (mut src_rel, mut out_rel) = (0i64, 0i64);
    let signed = |o: u64| if o & 1 == 1 { -((o >> 1) as i64) } else { (o >> 1) as i64 };
    while r.i < end {
        let d = r.num()?;
        let len = (d >> 2) as usize + 1;
        match d & 3 {
            0 => {
                let at = out.len();
                out.extend_from_slice(src.get(at..at + len).ok_or_else(bad)?);
            }
            1 => out.extend_from_slice(r.bytes(len)?),
            2 => {
                src_rel += signed(r.num()?);
                let s = usize::try_from(src_rel).map_err(|_| bad())?;
                out.extend_from_slice(src.get(s..s + len).ok_or_else(bad)?);
                src_rel += len as i64;
            }
            _ => {
                out_rel += signed(r.num()?);
                for _ in 0..len {
                    let b = *usize::try_from(out_rel).ok().and_then(|t| out.get(t)).ok_or_else(bad)?;
                    out.push(b);
                    out_rel += 1;
                }
            }
        }
    }
    if out.len() != size || crc32fast::hash(&out) != crc(end + 4) {
        return Err(bad());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ips_write_and_rle() {
        let mut p = b"PATCH".to_vec();
        p.extend([0, 0, 4, 0, 2, b'x', b'!']); // escribe "x!" en 4
        p.extend([0, 0, 0, 0, 0, 0, 2, b'H']); // RLE: "HH" en 0
        p.extend(b"EOF");
        assert_eq!(apply(&p, b"hello").unwrap(), b"HHllx!");
    }

    #[test]
    fn bps_all_actions() {
        let (src, tgt) = (b"hello", b"hellx!lo");
        let mut p = b"BPS1".to_vec();
        // tamaños 5 y 8, sin metadata; luego: SourceRead 4, TargetRead "x!", SourceCopy +3 len 2
        p.extend([0x85, 0x88, 0x80, (3 << 2) | 0x80, (1 << 2 | 1) | 0x80, b'x', b'!']);
        p.extend([(1 << 2 | 2) | 0x80, (3 << 1) | 0x80]);
        p.extend(crc32fast::hash(src).to_le_bytes());
        p.extend(crc32fast::hash(tgt).to_le_bytes());
        p.extend(crc32fast::hash(&p).to_le_bytes());
        assert_eq!(apply(&p, src).unwrap(), tgt);
        assert!(apply(&p, b"jello").is_err());
    }
}
