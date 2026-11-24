#![cfg(all(
    feature = "linux-elf",
    feature = "windows-pe",
    feature = "macho-inspect"
))]

use bifrost_runtime::process::inspect;

#[test]
fn image_inspection_handles_deterministic_mutations_and_truncations() {
    for fixture in [
        &include_bytes!("../../../tests/fixtures/linux/threads.elf")[..],
        &include_bytes!("../../../tests/fixtures/windows/hello.exe")[..],
        &include_bytes!("../../../tests/fixtures/macho/entry-x86_64.macho")[..],
        &include_bytes!("../../../tests/fixtures/macho/entry-arm64.macho")[..],
    ] {
        assert!(inspect(fixture).is_ok());
        let mut mutated = fixture.to_vec();
        let mut accepted = 0;
        let mut rejected = 0;
        // Walk the header, then sample the remaining sections and import tables.
        for offset in (0..fixture.len().min(768)).chain((768..fixture.len()).step_by(7)) {
            let original = fixture[offset];
            for value in [0, u8::MAX, original ^ 0x80] {
                mutated[offset] = value;
                if inspect(&mutated).is_ok() {
                    accepted += 1;
                } else {
                    rejected += 1;
                }
            }
            mutated[offset] = original;
        }
        assert!(accepted > 0 && rejected > 0);
        for end in (0..fixture.len().min(768)).chain((768..fixture.len()).step_by(19)) {
            let _ = inspect(&fixture[..end]);
        }
    }
}
