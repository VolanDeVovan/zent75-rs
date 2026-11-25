//! CRC-16/MODBUS (reflected poly 0xA001, init 0xFFFF) used by the configurator protocol.

pub fn crc16_modbus(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_value() {
        // Standard check value of CRC-16/MODBUS.
        assert_eq!(crc16_modbus(b"123456789"), 0x4B37);
    }
}
