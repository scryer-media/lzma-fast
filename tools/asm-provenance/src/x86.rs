//! Instruction-level comparison of two x86-64 functions.
//!
//! Two assemblers given the same source can emit different bytes for it:
//! either operand order for a register-to-register `mov`, an 8-bit or a
//! 32-bit displacement or branch offset, and whatever padding fills an
//! alignment directive. None of that changes what runs. What must not differ
//! is the sequence of instructions and where each branch goes, so both
//! functions are decoded, NOPs dropped, and every branch target rewritten as
//! the index of the instruction it lands on. The rendered text of the two
//! streams must then be equal line for line.

use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter, Mnemonic, OpKind};

pub struct Report {
    pub instructions: usize,
    pub encodings_differ: usize,
    pub nops_ours: usize,
    pub nops_theirs: usize,
}

struct Decoded {
    /// The rendered, position-independent form of each non-NOP instruction.
    lines: Vec<String>,
    /// Each non-NOP instruction's encoding.
    encodings: Vec<Vec<u8>>,
    nops: usize,
}

pub fn compare(ours: &[u8], theirs: &[u8]) -> Result<Report, String> {
    let a = decode(ours).map_err(|e| format!("    ours: {e}"))?;
    let b = decode(theirs).map_err(|e| format!("    theirs: {e}"))?;

    if let Some(at) =
        (0..a.lines.len().max(b.lines.len())).find(|&i| a.lines.get(i) != b.lines.get(i))
    {
        let mut message = format!(
            "    instruction {at} differs ({} vs {} instructions after dropping NOPs)\n",
            a.lines.len(),
            b.lines.len()
        );
        for i in at.saturating_sub(4)..(at + 6).min(a.lines.len().max(b.lines.len())) {
            let mark = if a.lines.get(i) == b.lines.get(i) {
                ' '
            } else {
                '!'
            };
            message.push_str(&format!(
                "    {mark} {i:5}  {:<40} {}\n",
                a.lines.get(i).map_or("<end>", String::as_str),
                b.lines.get(i).map_or("<end>", String::as_str),
            ));
        }
        return Err(message);
    }

    Ok(Report {
        instructions: a.lines.len(),
        encodings_differ: a
            .encodings
            .iter()
            .zip(&b.encodings)
            .filter(|(x, y)| x != y)
            .count(),
        nops_ours: a.nops,
        nops_theirs: b.nops,
    })
}

fn is_nop(instruction: &Instruction) -> bool {
    instruction.mnemonic() == Mnemonic::Nop
}

fn is_branch(instruction: &Instruction) -> bool {
    matches!(
        instruction.op0_kind(),
        OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64
    )
}

fn decode(bytes: &[u8]) -> Result<Decoded, String> {
    let mut decoder = Decoder::with_ip(64, bytes, 0, DecoderOptions::NONE);
    let mut all = Vec::new();
    while decoder.can_decode() {
        let instruction = decoder.decode();
        if instruction.is_invalid() {
            return Err(format!("undecodable bytes at 0x{:x}", instruction.ip()));
        }
        all.push(instruction);
    }

    // Instruction boundaries, and for each one the index its non-NOP
    // successor will have once NOPs are dropped: a branch into padding lands
    // on whatever the padding falls through to.
    let mut index_at = std::collections::BTreeMap::new();
    let mut next_index = 0usize;
    for instruction in &all {
        index_at.insert(instruction.ip(), next_index);
        if !is_nop(instruction) {
            next_index += 1;
        }
    }
    // A branch to the byte just past the function is a branch to its end.
    index_at.insert(bytes.len() as u64, next_index);

    let mut formatter = IntelFormatter::new();
    formatter.options_mut().set_uppercase_hex(false);
    formatter
        .options_mut()
        .set_space_after_operand_separator(true);

    let mut lines = Vec::new();
    let mut encodings = Vec::new();
    let mut nops = 0;
    for instruction in &all {
        if is_nop(instruction) {
            nops += 1;
            continue;
        }
        if instruction.is_ip_rel_memory_operand() {
            return Err(format!(
                "RIP-relative operand at 0x{:x}; the loop should reference no data",
                instruction.ip()
            ));
        }
        let mut line = String::new();
        if is_branch(instruction) {
            let target = instruction.near_branch_target();
            let index = index_at.get(&target).ok_or_else(|| {
                format!(
                    "branch at 0x{:x} targets 0x{target:x}, which is not an instruction boundary in the function",
                    instruction.ip()
                )
            })?;
            formatter.format_mnemonic(instruction, &mut line);
            line.push_str(&format!(" @{index}"));
        } else {
            formatter.format(instruction, &mut line);
        }
        lines.push(line);
        let start = instruction.ip() as usize;
        encodings.push(bytes[start..start + instruction.len()].to_vec());
    }
    Ok(Decoded {
        lines,
        encodings,
        nops,
    })
}

#[cfg(test)]
mod tests {
    use super::compare;

    #[test]
    fn two_encodings_of_one_instruction_are_the_same_instruction() {
        // mov eax,ecx as 89 /r and as 8B /r, then ret.
        let report = compare(&[0x89, 0xc8, 0xc3], &[0x8b, 0xc1, 0xc3]).expect("equal");
        assert_eq!(report.instructions, 2);
        assert_eq!(report.encodings_differ, 1);
    }

    #[test]
    fn a_short_and_a_near_branch_to_the_same_instruction_agree() {
        // jne +1 over an int3, as rel8 and as rel32.
        let short = [0x75, 0x01, 0xcc, 0xc3];
        let near = [0x0f, 0x85, 0x01, 0x00, 0x00, 0x00, 0xcc, 0xc3];
        compare(&short, &near).expect("same branch target");
    }

    #[test]
    fn padding_is_not_an_instruction() {
        // jmp over 3 bytes of NOP padding to a ret, against a jmp straight to it.
        let padded = [0xeb, 0x03, 0x0f, 0x1f, 0x00, 0xc3];
        let tight = [0xeb, 0x00, 0xc3];
        let report = compare(&padded, &tight).expect("NOPs dropped");
        assert_eq!((report.nops_ours, report.nops_theirs), (1, 0));
    }

    #[test]
    fn a_branch_to_a_different_instruction_is_caught() {
        // jne +0 lands on the int3, jne +1 on the ret.
        let a = [0x75, 0x00, 0xcc, 0xc3];
        let b = [0x75, 0x01, 0xcc, 0xc3];
        assert!(compare(&a, &b).is_err());
    }

    #[test]
    fn a_changed_operand_is_caught() {
        // shr edi,5 against shr edi,4.
        assert!(compare(&[0xc1, 0xef, 0x05, 0xc3], &[0xc1, 0xef, 0x04, 0xc3]).is_err());
    }

    #[test]
    fn a_missing_instruction_is_caught() {
        assert!(compare(&[0x90, 0x55, 0xc3], &[0x90, 0xc3]).is_err());
    }

    #[test]
    fn a_branch_into_the_middle_of_an_instruction_is_refused() {
        // jmp +1 lands inside the two-byte mov.
        assert!(
            compare(
                &[0xeb, 0x01, 0x89, 0xc8, 0xc3],
                &[0xeb, 0x01, 0x89, 0xc8, 0xc3]
            )
            .is_err()
        );
    }
}
