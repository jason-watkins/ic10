/// Compute the ISO 3309 / ITU-T V.42 CRC-32 of a UTF-8 string, as used by
/// the Stationeers IC10 `HASH` instruction and `hash()` expressions in IC20.
///
/// The value is returned as an `f64` because all IC10 register values are
/// IEEE 754 doubles; 32-bit unsigned integers are always representable exactly.
pub fn crc32(s: &str) -> f64 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for byte in s.bytes() {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    f64::from((crc ^ 0xFFFF_FFFF) as i32)
}

#[cfg(test)]
mod tests {
    use super::crc32;

    #[test]
    fn matches_game_known_values() {
        assert_eq!(crc32("UniformOrangeJumpSuit"), 810053150.0);
        assert_eq!(crc32("ItemEvaSuit"), 1677018918.0);
        assert_eq!(crc32("ItemIronOre"), 1758427767.0);
        assert_eq!(crc32("StructureElectronicsPrinter"), 1307165496.0);
        assert_eq!(crc32("StructureSolarPanel"), -2045627372.0);
        assert_eq!(crc32("StructureSolarPanelDual"), -539224550.0);
    }
}
