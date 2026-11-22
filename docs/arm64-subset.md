# ARM64 scalar guest profile

The raw profile uses little-endian A64 instructions and serialized execution over owned normal guest memory. Every instruction fetch requires four-byte PC alignment and execute permission. The decoder preserves the instruction word and PC. It reports words outside the listed profile as unsupported. UDF produces a guest illegal-instruction fault.

| Family | Accepted forms |
| --- | --- |
| MOV | MOVZ, MOVN and MOVK to X registers, and MOV from a register via the unshifted ORR alias |
| Arithmetic | 64-bit ADD, ADDS, SUB, SUBS with imm12 (optional LSL #12) or register LSL #0, including CMP/CMN and immediate moves to/from SP |
| Memory | 64-bit unsigned scaled immediate LDR/STR with Xn base, including unaligned normal-data accesses |
| Branch | B, BL, B.cond, 64-bit CBZ/CBNZ, RET Xn |
| Miscellaneous | NOP, DMB ISH, UDF |

Register 31 denotes SP where the immediate arithmetic encoding permits it and XZR in the listed zero-register forms. ADDS/SUBS set N, Z, C and V without changing SP. SUBS sets C when there is no unsigned borrow. BL writes the next guest PC to X30, and RET reads its destination register. The next fetch detects a misaligned return destination.

The memory profile orders guest accesses within a serialized execution session. DMB ISH explicitly fences both backends. SP-relative memory, atomics/exclusives, parallel weak-memory outcomes, vector instructions, privilege and OS services are outside this profile. The baseline profile requires no optional ISA features. [Armv8-A instruction set guide](https://developer.arm.com/-/media/Arm%20Developer%20Community/PDF/Learn%20the%20Architecture/Armv8-A%20Instruction%20Set%20Architecture.pdf)
