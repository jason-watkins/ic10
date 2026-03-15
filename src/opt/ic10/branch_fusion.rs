use crate::regalloc::ic10::{IC10Instruction, JumpTarget, Operand, Register};

#[derive(Clone, Copy)]
enum ComparisonKind {
    Equal,
    NotEqual,
    GreaterThan,
    GreaterEqual,
    LessThan,
    LessEqual,
}

struct Comparison {
    destination: Register,
    kind: ComparisonKind,
    left: Operand,
    right: Option<Operand>,
}

fn extract_comparison(instruction: &IC10Instruction) -> Option<Comparison> {
    match instruction {
        IC10Instruction::Seq(dest, left, right) => Some(Comparison {
            destination: *dest,
            kind: ComparisonKind::Equal,
            left: left.clone(),
            right: Some(right.clone()),
        }),
        IC10Instruction::Sne(dest, left, right) => Some(Comparison {
            destination: *dest,
            kind: ComparisonKind::NotEqual,
            left: left.clone(),
            right: Some(right.clone()),
        }),
        IC10Instruction::Sgt(dest, left, right) => Some(Comparison {
            destination: *dest,
            kind: ComparisonKind::GreaterThan,
            left: left.clone(),
            right: Some(right.clone()),
        }),
        IC10Instruction::Sge(dest, left, right) => Some(Comparison {
            destination: *dest,
            kind: ComparisonKind::GreaterEqual,
            left: left.clone(),
            right: Some(right.clone()),
        }),
        IC10Instruction::Slt(dest, left, right) => Some(Comparison {
            destination: *dest,
            kind: ComparisonKind::LessThan,
            left: left.clone(),
            right: Some(right.clone()),
        }),
        IC10Instruction::Sle(dest, left, right) => Some(Comparison {
            destination: *dest,
            kind: ComparisonKind::LessEqual,
            left: left.clone(),
            right: Some(right.clone()),
        }),
        IC10Instruction::Seqz(dest, operand) => Some(Comparison {
            destination: *dest,
            kind: ComparisonKind::Equal,
            left: operand.clone(),
            right: None,
        }),
        IC10Instruction::Snez(dest, operand) => Some(Comparison {
            destination: *dest,
            kind: ComparisonKind::NotEqual,
            left: operand.clone(),
            right: None,
        }),
        IC10Instruction::Sgtz(dest, operand) => Some(Comparison {
            destination: *dest,
            kind: ComparisonKind::GreaterThan,
            left: operand.clone(),
            right: None,
        }),
        IC10Instruction::Sgez(dest, operand) => Some(Comparison {
            destination: *dest,
            kind: ComparisonKind::GreaterEqual,
            left: operand.clone(),
            right: None,
        }),
        IC10Instruction::Sltz(dest, operand) => Some(Comparison {
            destination: *dest,
            kind: ComparisonKind::LessThan,
            left: operand.clone(),
            right: None,
        }),
        IC10Instruction::Slez(dest, operand) => Some(Comparison {
            destination: *dest,
            kind: ComparisonKind::LessEqual,
            left: operand.clone(),
            right: None,
        }),
        _ => None,
    }
}

fn invert(kind: ComparisonKind) -> ComparisonKind {
    match kind {
        ComparisonKind::Equal => ComparisonKind::NotEqual,
        ComparisonKind::NotEqual => ComparisonKind::Equal,
        ComparisonKind::GreaterThan => ComparisonKind::LessEqual,
        ComparisonKind::LessEqual => ComparisonKind::GreaterThan,
        ComparisonKind::LessThan => ComparisonKind::GreaterEqual,
        ComparisonKind::GreaterEqual => ComparisonKind::LessThan,
    }
}

fn build_fused_branch(
    kind: ComparisonKind,
    left: Operand,
    right: Option<Operand>,
    target: JumpTarget,
) -> IC10Instruction {
    match (kind, right) {
        (ComparisonKind::Equal, Some(right)) => IC10Instruction::BranchEqual(left, right, target),
        (ComparisonKind::Equal, None) => IC10Instruction::BranchEqualZero(left, target),
        (ComparisonKind::NotEqual, Some(right)) => {
            IC10Instruction::BranchNotEqual(left, right, target)
        }
        (ComparisonKind::NotEqual, None) => IC10Instruction::BranchNotEqualZero(left, target),
        (ComparisonKind::GreaterThan, Some(right)) => {
            IC10Instruction::BranchGreaterThan(left, right, target)
        }
        (ComparisonKind::GreaterThan, None) => IC10Instruction::BranchGreaterThanZero(left, target),
        (ComparisonKind::GreaterEqual, Some(right)) => {
            IC10Instruction::BranchGreaterEqual(left, right, target)
        }
        (ComparisonKind::GreaterEqual, None) => {
            IC10Instruction::BranchGreaterEqualZero(left, target)
        }
        (ComparisonKind::LessThan, Some(right)) => {
            IC10Instruction::BranchLessThan(left, right, target)
        }
        (ComparisonKind::LessThan, None) => IC10Instruction::BranchLessThanZero(left, target),
        (ComparisonKind::LessEqual, Some(right)) => {
            IC10Instruction::BranchLessEqual(left, right, target)
        }
        (ComparisonKind::LessEqual, None) => IC10Instruction::BranchLessEqualZero(left, target),
    }
}

fn extract_branch_on_register(
    instruction: &IC10Instruction,
    register: &Register,
) -> Option<(bool, JumpTarget)> {
    match instruction {
        IC10Instruction::BranchNotEqualZero(Operand::Register(r), target) if r == register => {
            Some((true, target.clone()))
        }
        IC10Instruction::BranchEqualZero(Operand::Register(r), target) if r == register => {
            Some((false, target.clone()))
        }
        _ => None,
    }
}

fn operand_uses_register(operand: &Operand, register: &Register) -> bool {
    matches!(operand, Operand::Register(r) if r == register)
}

/// Fuse `s??(z) rD ... ; b(n)eqz rD target` sequences into single conditional branches.
///
/// The emitter always produces the two-instruction pattern: a set-comparison followed by
/// a branch-on-zero/nonzero testing the comparison result. This pass collapses each such
/// pair into a single fused branch instruction (e.g. `sgt r0 r1 r2 ; bnez r0 L` → `bgt r1 r2 L`).
///
/// Push/pop instructions (from pressure spills at the terminator position) may appear between
/// the comparison and branch. These are safe to skip as long as no `Pop` overwrites a
/// register that the comparison reads — the fused branch will execute after those push/pop
/// and needs the original operand values intact.
///
/// The comparison result register is always dead after the branch: by SSA construction,
/// its only use is the branch condition, and the register allocator ends its live range
/// at the terminator.
pub(super) fn fuse_branches(instructions: &mut Vec<IC10Instruction>) {
    let mut result = Vec::with_capacity(instructions.len());
    let mut index = 0;

    while index < instructions.len() {
        if let Some(comparison) = extract_comparison(&instructions[index])
            && let Some((intervening_end, fused)) = try_fuse(instructions, index, &comparison)
        {
            result.extend_from_slice(&instructions[(index + 1)..intervening_end]);
            result.push(fused);
            index = intervening_end + 1;
            continue;
        }
        result.push(instructions[index].clone());
        index += 1;
    }

    *instructions = result;
}

/// Try to fuse the comparison at `comparison_index` with a subsequent branch.
///
/// Returns the index of the branch instruction and the fused replacement, or `None`
/// if the pattern doesn't match.
fn try_fuse(
    instructions: &[IC10Instruction],
    comparison_index: usize,
    comparison: &Comparison,
) -> Option<(usize, IC10Instruction)> {
    let mut scan = comparison_index + 1;
    while scan < instructions.len() {
        match &instructions[scan] {
            IC10Instruction::Push(_) => {}
            IC10Instruction::Pop(register) => {
                if operand_uses_register(&comparison.left, register)
                    || comparison
                        .right
                        .as_ref()
                        .is_some_and(|r| operand_uses_register(r, register))
                {
                    return None;
                }
            }
            _ => break,
        }
        scan += 1;
    }

    if scan >= instructions.len() {
        return None;
    }

    let (is_nonzero, target) =
        extract_branch_on_register(&instructions[scan], &comparison.destination)?;

    let kind = if is_nonzero {
        comparison.kind
    } else {
        invert(comparison.kind)
    };

    let fused = build_fused_branch(
        kind,
        comparison.left.clone(),
        comparison.right.clone(),
        target,
    );
    Some((scan, fused))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn register(n: u8) -> Register {
        match n {
            0 => Register::R0,
            1 => Register::R1,
            2 => Register::R2,
            3 => Register::R3,
            4 => Register::R4,
            _ => panic!("test register out of range"),
        }
    }

    fn reg(n: u8) -> Operand {
        Operand::Register(register(n))
    }

    fn label(name: &str) -> JumpTarget {
        JumpTarget::Label(name.to_string())
    }

    #[test]
    fn fuse_sgt_bnez() {
        let mut instructions = vec![
            IC10Instruction::Sgt(register(0), reg(1), reg(2)),
            IC10Instruction::BranchNotEqualZero(reg(0), label("target")),
        ];
        fuse_branches(&mut instructions);
        assert_eq!(instructions.len(), 1);
        assert!(matches!(
            &instructions[0],
            IC10Instruction::BranchGreaterThan(
                Operand::Register(Register::R1),
                Operand::Register(Register::R2),
                JumpTarget::Label(name),
            ) if name == "target"
        ));
    }

    #[test]
    fn fuse_sgt_beqz_inverts() {
        let mut instructions = vec![
            IC10Instruction::Sgt(register(0), reg(1), reg(2)),
            IC10Instruction::BranchEqualZero(reg(0), label("target")),
        ];
        fuse_branches(&mut instructions);
        assert_eq!(instructions.len(), 1);
        assert!(matches!(
            &instructions[0],
            IC10Instruction::BranchLessEqual(
                Operand::Register(Register::R1),
                Operand::Register(Register::R2),
                JumpTarget::Label(name),
            ) if name == "target"
        ));
    }

    #[test]
    fn fuse_slt_bnez_with_literal() {
        let mut instructions = vec![
            IC10Instruction::Slt(register(0), reg(1), Operand::Literal(5.0)),
            IC10Instruction::BranchNotEqualZero(reg(0), label("target")),
        ];
        fuse_branches(&mut instructions);
        assert_eq!(instructions.len(), 1);
        assert!(matches!(
            &instructions[0],
            IC10Instruction::BranchLessThan(
                Operand::Register(Register::R1),
                Operand::Literal(v),
                JumpTarget::Label(name),
            ) if *v == 5.0 && name == "target"
        ));
    }

    #[test]
    fn fuse_skips_safe_push_pop() {
        let mut instructions = vec![
            IC10Instruction::Sgt(register(0), reg(1), reg(2)),
            IC10Instruction::Push(reg(3)),
            IC10Instruction::Pop(register(4)),
            IC10Instruction::BranchNotEqualZero(reg(0), label("target")),
        ];
        fuse_branches(&mut instructions);
        assert_eq!(instructions.len(), 3);
        assert!(matches!(instructions[0], IC10Instruction::Push(_)));
        assert!(matches!(instructions[1], IC10Instruction::Pop(_)));
        assert!(matches!(
            &instructions[2],
            IC10Instruction::BranchGreaterThan(..)
        ));
    }

    #[test]
    fn no_fuse_when_pop_clobbers_operand() {
        let mut instructions = vec![
            IC10Instruction::Sgt(register(0), reg(1), reg(2)),
            IC10Instruction::Pop(register(2)),
            IC10Instruction::BranchNotEqualZero(reg(0), label("target")),
        ];
        let original_len = instructions.len();
        fuse_branches(&mut instructions);
        assert_eq!(instructions.len(), original_len);
    }

    #[test]
    fn no_fuse_when_branch_uses_different_register() {
        let mut instructions = vec![
            IC10Instruction::Sgt(register(0), reg(1), reg(2)),
            IC10Instruction::BranchNotEqualZero(reg(3), label("target")),
        ];
        let original_len = instructions.len();
        fuse_branches(&mut instructions);
        assert_eq!(instructions.len(), original_len);
    }

    #[test]
    fn no_fuse_when_non_push_pop_intervenes() {
        let mut instructions = vec![
            IC10Instruction::Sgt(register(0), reg(1), reg(2)),
            IC10Instruction::Move(register(3), reg(4)),
            IC10Instruction::BranchNotEqualZero(reg(0), label("target")),
        ];
        let original_len = instructions.len();
        fuse_branches(&mut instructions);
        assert_eq!(instructions.len(), original_len);
    }

    #[test]
    fn fuse_zero_variant_sgtz_bnez() {
        let mut instructions = vec![
            IC10Instruction::Sgtz(register(0), reg(1)),
            IC10Instruction::BranchNotEqualZero(reg(0), label("target")),
        ];
        fuse_branches(&mut instructions);
        assert_eq!(instructions.len(), 1);
        assert!(matches!(
            &instructions[0],
            IC10Instruction::BranchGreaterThanZero(
                Operand::Register(Register::R1),
                JumpTarget::Label(name),
            ) if name == "target"
        ));
    }

    #[test]
    fn fuse_seq_beqz_inverts_to_bne() {
        let mut instructions = vec![
            IC10Instruction::Seq(register(0), reg(1), reg(2)),
            IC10Instruction::BranchEqualZero(reg(0), label("target")),
        ];
        fuse_branches(&mut instructions);
        assert_eq!(instructions.len(), 1);
        assert!(matches!(
            &instructions[0],
            IC10Instruction::BranchNotEqual(
                Operand::Register(Register::R1),
                Operand::Register(Register::R2),
                JumpTarget::Label(name),
            ) if name == "target"
        ));
    }

    #[test]
    fn fuse_multiple_in_sequence() {
        let mut instructions = vec![
            IC10Instruction::Sgt(register(0), reg(1), reg(2)),
            IC10Instruction::BranchNotEqualZero(reg(0), label("a")),
            IC10Instruction::Slt(register(0), reg(3), reg(4)),
            IC10Instruction::BranchNotEqualZero(reg(0), label("b")),
        ];
        fuse_branches(&mut instructions);
        assert_eq!(instructions.len(), 2);
        assert!(matches!(
            &instructions[0],
            IC10Instruction::BranchGreaterThan(..)
        ));
        assert!(matches!(
            &instructions[1],
            IC10Instruction::BranchLessThan(..)
        ));
    }
}
