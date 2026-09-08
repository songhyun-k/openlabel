use crate::{Error, Result, settings::Printer};
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub bytes: Vec<u8>,
    pub delay_ms: u64,
}
pub fn sequence(bits: &[u8], rows: u16, printer: &Printer) -> Result<Vec<Step>> {
    if rows == 0
        || rows > 800
        || bits.len() != usize::from(rows) * 48
        || !(1..=15).contains(&printer.density)
        || !(1..=5).contains(&printer.speed)
    {
        return Err(Error::localized("invalid_settings", "err.m110Range", &[]));
    }
    let mut steps = vec![
        Step {
            bytes: vec![0x1b, 0x4e, 0x0d, printer.speed],
            delay_ms: 30,
        },
        Step {
            bytes: vec![0x1b, 0x4e, 0x04, printer.density],
            delay_ms: 30,
        },
        Step {
            bytes: vec![0x1f, 0x11, 0x0a],
            delay_ms: 30,
        },
        Step {
            bytes: vec![0x1d, 0x76, 0x30, 0, 48, 0, rows as u8, (rows >> 8) as u8],
            delay_ms: 0,
        },
    ];
    for chunk in bits.chunks(128) {
        steps.push(Step {
            bytes: chunk.to_vec(),
            delay_ms: 20,
        });
    }
    steps.push(Step {
        bytes: vec![],
        delay_ms: 300,
    });
    steps.push(Step {
        bytes: vec![0x1f, 0xf0, 5, 0, 0x1f, 0xf0, 3, 0],
        delay_ms: 500,
    });
    Ok(steps)
}
