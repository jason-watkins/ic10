use crate::regalloc::ic10::{IC10Instruction, Operand};

pub(super) fn simplify_instructions(instructions: &mut Vec<IC10Instruction>) {
    eliminate_self_moves(instructions);
    simplify_arithmetic_identities(instructions);
}

fn eliminate_self_moves(instructions: &mut Vec<IC10Instruction>) {
    instructions.retain(|instruction| {
        !matches!(
            instruction,
            IC10Instruction::Move(dest, Operand::Register(source)) if dest == source
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
            IC10Instruction::Add(dest, left, right) if is_zero(right) => {
                Some(IC10Instruction::Move(*dest, left.clone()))
            }
            IC10Instruction::Add(dest, left, right) if is_zero(left) => {
                Some(IC10Instruction::Move(*dest, right.clone()))
            }
            IC10Instruction::Sub(dest, left, right) if is_zero(right) => {
                Some(IC10Instruction::Move(*dest, left.clone()))
            }
            IC10Instruction::Mul(dest, left, right) if is_one(right) => {
                Some(IC10Instruction::Move(*dest, left.clone()))
            }
            IC10Instruction::Mul(dest, left, right) if is_one(left) => {
                Some(IC10Instruction::Move(*dest, right.clone()))
            }
            IC10Instruction::Mul(dest, _, right) if is_zero(right) => {
                Some(IC10Instruction::Move(*dest, Operand::Literal(0.0)))
            }
            IC10Instruction::Mul(dest, left, _) if is_zero(left) => {
                Some(IC10Instruction::Move(*dest, Operand::Literal(0.0)))
            }
            IC10Instruction::Div(dest, left, right) if is_one(right) => {
                Some(IC10Instruction::Move(*dest, left.clone()))
            }
            _ => None,
        };
        if let Some(new_instruction) = replacement {
            *instruction = new_instruction;
        }
    }
}
