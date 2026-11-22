# Initial x86-64 instruction subset

The first execution profile uses bare x86-64 long mode, full-width general-purpose registers, and mapped, canonical 48-bit guest virtual addresses. Instructions use the listed encodings with ordinary REX prefixes. Legacy prefixes, address-size overrides, register subparts, and segment overrides are outside this profile.

| Family | Accepted forms |
| --- | --- |
| NOP | Single-byte opcode `90` as decoded as NOP |
| MOV | `r64, r64`, `r64, imm64` or sign-extended `imm32`, and ModRM forms `r64, m64`, `m64, r64`, `m64, imm32` sign-extended |
| LEA | `r64, m` computes the 64-bit effective address without reading guest memory |
| ADD, SUB, CMP | `r64, r64` or sign-extended `imm8/imm32`, including accumulator forms |
| JMP, JE, JNE, JB, JAE, JL, JGE | Direct short and near relative branches |
| CALL, RET | Near relative and register/memory indirect CALL, and plain near RET, with 8-byte stack slots |
| UD2 | Deliberate invalid-instruction fault |

Memory operands use 64-bit addressing, including SIB and RIP-relative addressing. Displacements and index arithmetic wrap at 64 bits. Unaligned scalar accesses are permitted. CALL writes the next RIP at RSP minus 8 before updating RSP. RET loads the target from RSP before advancing it. A failed access leaves the instruction's architectural state unchanged.

ADD, SUB, and CMP set CF, PF, AF, ZF, SF, and OF. PF is the even parity of the result's low byte. CMP updates flags without storing its result. Other supported instructions preserve flags. JE/JNE test ZF, JB/JAE test CF, and JL/JGE compare SF and OF. An unsupported valid instruction produces a typed error. Invalid encoding, instruction fetch failures, and execution faults identify the guest PC.

The deterministic fixture vectors live in `bifrost-testkit`. They record the machine bytes, initial register values, and expected effects independently of the decoder and lifter.

The differential tests use fixed native assembly on x86-64 hosts and independent Rust expectations on Linux ARM64. They compare the interpreter and JIT for 4,860 arithmetic cases and 468 conditional branch cases, plus memory, RIP-relative addressing, jumps and CALL/RET stack fixtures. Guest-fault fixtures compare interpreter and JIT state and trace PCs, including UD2, protection and unmapped accesses.
