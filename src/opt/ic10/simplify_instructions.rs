use crate::regalloc::ic10::{IC10Instruction, Operand};

/// Eliminates self-moves (`move rX rX`) and rewrites arithmetic identity
/// patterns (e.g. `add rX rY 0` → `move rX rY`) on the flat IC10 instruction
/// stream.
pub(super) fn simplify_instructions(instructions: &mut Vec<IC10Instruction>) {
    eliminate_self_moves(instructions);
    simplify_arithmetic_identities(instructions);
}

fn eliminate_self_moves(instructions: &mut Vec<IC10Instruction>) {
    instructions.retain(|instruction| {
        !matches!(
            instruction,
            IC10Instruction::Move(target, Operand::Register(source)) if target == source
        )
    });
}

fn is_zero(operand: &Operand) -> bool {
    matches!(operand, Operand::Literal(v) if *v == 0.0)
}

fn is_one(operand: &Operand) -> bool {
    matches!(operand, Operand::Literal(v) if *v == 1.0)
}

fn simplify_arithmetic_identities(instructions: &mut [IC10Instruction]) {
    for instruction in instructions.iter_mut() {
        let replacement = match instruction {
            IC10Instruction::Add(target, left, right) if is_zero(right) => {
                Some(IC10Instruction::Move(*target, left.clone()))
            }
            IC10Instruction::Add(target, left, right) if is_zero(left) => {
                Some(IC10Instruction::Move(*target, right.clone()))
            }
            IC10Instruction::Sub(target, left, right) if is_zero(right) => {
                Some(IC10Instruction::Move(*target, left.clone()))
            }
            IC10Instruction::Mul(target, left, right) if is_one(right) => {
                Some(IC10Instruction::Move(*target, left.clone()))
            }
            IC10Instruction::Mul(target, left, right) if is_one(left) => {
                Some(IC10Instruction::Move(*target, right.clone()))
            }
            IC10Instruction::Mul(target, _, right) if is_zero(right) => {
                Some(IC10Instruction::Move(*target, Operand::Literal(0.0)))
            }
            IC10Instruction::Mul(target, left, _) if is_zero(left) => {
                Some(IC10Instruction::Move(*target, Operand::Literal(0.0)))
            }
            IC10Instruction::Div(target, left, right) if is_one(right) => {
                Some(IC10Instruction::Move(*target, left.clone()))
            }
            _ => None,
        };
        if let Some(new_instruction) = replacement {
            *instruction = new_instruction;
        }
    }
}
