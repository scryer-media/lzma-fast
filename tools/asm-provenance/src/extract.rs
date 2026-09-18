//! Pulling the decode loop out of an object file.

use std::{fs, path::Path};

use object::{Object, ObjectSection, ObjectSymbol, SectionKind, SymbolKind};

/// The loop's bytes as the object holds them.
pub struct Function {
    pub bytes: Vec<u8>,
}

/// The crate's loop: the one defined symbol whose name carries the Rust
/// function's, `lzma_dec_decode_real_3`, in any mangling or decoration.
pub fn ours(path: &Path) -> Result<Function, String> {
    extract(path, |name| name.contains("lzma_dec_decode_real_3"), false)
}

/// The SDK's loop, `LzmaDec_DecodeReal_3`, with or without the leading
/// underscore some object formats add.
pub fn reference(path: &Path) -> Result<Function, String> {
    extract(
        path,
        |name| name.trim_start_matches('_') == "LzmaDec_DecodeReal_3",
        true,
    )
}

/// `labels_are_internal`: the SDK objects hold nothing but the loop, and the
/// JWasm family and GNU as leave its labels in the symbol table as locals, so
/// only a global symbol may end the function there. The crate's objects hold
/// the whole library, and its assembly labels are assembler-temporary, so
/// there the next symbol of any kind ends it.
fn extract(
    path: &Path,
    wanted: impl Fn(&str) -> bool,
    labels_are_internal: bool,
) -> Result<Function, String> {
    let data = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let file = object::File::parse(&*data).map_err(|e| format!("parse {}: {e}", path.display()))?;

    // By section, not by symbol type: the SDK's arm64 file leaves its
    // `.type ..., %function` line commented out, so its symbol has none.
    let in_code = |s: &object::Symbol| {
        s.section_index()
            .and_then(|i| file.section_by_index(i).ok())
            .is_some_and(|section| section.kind() == SectionKind::Text)
    };
    let mut matches = file
        .symbols()
        .filter(|s| s.kind() != SymbolKind::Section && s.kind() != SymbolKind::File)
        .filter(|s| !s.is_undefined() && in_code(s))
        .filter(|s| s.name().is_ok_and(&wanted))
        .collect::<Vec<_>>();
    // The SDK's arm64 file also defines a local `_LzmaDec_DecodeReal_3` label
    // at the entry; the exported symbol is the one wanted.
    if matches.len() > 1 {
        matches.retain(|s| s.is_global());
    }
    let symbol = match matches.as_slice() {
        [symbol] => symbol,
        [] => {
            return Err(format!(
                "{}: the decode loop symbol is not defined",
                path.display()
            ));
        }
        _ => {
            return Err(format!(
                "{}: more than one decode loop symbol",
                path.display()
            ));
        }
    };

    let section_index = symbol
        .section_index()
        .ok_or_else(|| format!("{}: the loop symbol has no section", path.display()))?;
    let section = file
        .section_by_index(section_index)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let contents = section
        .data()
        .map_err(|e| format!("{}: section data: {e}", path.display()))?;

    let start = symbol.address() - section.address();
    let mut end = section.size();
    if symbol.size() > 0 {
        end = start + symbol.size();
    } else {
        // Mach-O and COFF record no sizes; the next symbol in the section
        // bounds the function instead.
        for other in file.symbols() {
            if other.section_index() != Some(section_index) || other.address() <= symbol.address() {
                continue;
            }
            if labels_are_internal && !other.is_global() {
                continue;
            }
            let name = other.name().unwrap_or("");
            // Mach-O section-start markers, and anything assembler-temporary.
            if name.starts_with("ltmp") || name.starts_with('L') && !other.is_global() {
                continue;
            }
            end = end.min(other.address() - section.address());
        }
    }

    let relocations = section
        .relocations()
        .filter(|(offset, _)| (start..end).contains(offset))
        .count();
    if relocations > 0 {
        return Err(format!(
            "{}: the loop carries {relocations} relocation(s); it should be self-contained",
            path.display()
        ));
    }

    let bytes = contents
        .get(start as usize..end as usize)
        .ok_or_else(|| format!("{}: the loop runs past its section", path.display()))?
        .to_vec();
    Ok(Function { bytes })
}
