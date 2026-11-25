# ARM64 scalar guest profile

The raw profile uses little-endian A64 instructions and serialized execution over owned normal guest memory. Every instruction fetch requires four-byte PC alignment and execute permission. The decoder preserves the instruction word and PC. It reports words outside the listed profile as unsupported. UDF produces a guest illegal-instruction fault.

| Family | Accepted forms |
| --- | --- |
| MOV | MOVZ, MOVN and MOVK to W or X registers, and MOV from a W or X register via the unshifted ORR alias |
| Arithmetic | 32-bit and 64-bit ADD, ADDS, SUB, SUBS with imm12 (optional LSL #12) or register LSL #0, including CMP/CMN and immediate moves to/from WSP or SP |
| Multiply | 32-bit and 64-bit MADD and MSUB, including MUL and MNEG |
| Address generation | ADR and ADRP |
| Logical | 32-bit and 64-bit AND, ORR, EOR and ANDS with a bitmask immediate or shifted register, plus shifted-register BIC, ORN, EON and BICS |
| Bitfield | 32-bit and 64-bit BFM, SBFM and UBFM, including immediate shift and sign-extension aliases |
| Bit operations | 32-bit and 64-bit RBIT, CLZ, and CLS |
| Variable shifts | 32-bit and 64-bit LSLV, LSRV, ASRV and RORV |
| Memory | 32-bit and 64-bit unsigned scaled immediate and signed pre/post-indexed LDR/STR, signed unscaled immediate LDUR/STUR, unsigned immediate LDRB/STRB, and signed unscaled immediate LDURB/STURB with Xn or SP base |
| Register-offset memory | Unsigned byte, halfword, word and doubleword loads and stores with Xn or SP base and UXTW, SXTW, SXTX or LSL offset |
| Paired memory | 32-bit and 64-bit signed-offset LDP/STP with Xn or SP base, distinct LDP destinations, and no writeback |
| Branch | B, BL, B.cond, 32-bit and 64-bit CBZ/CBNZ, TBZ/TBNZ, RET Xn |
| Miscellaneous | NOP, DMB ISH, UDF |
| Supervisor call | SVC with a 16-bit immediate, reported to the caller as an execution exit |
| Pointer authentication | Register PAC/AUT, PACIASP/PACIBSP, AUTIASP/AUTIBSP, and RETAA/RETAB produce typed host exits before modifying guest state |

Register 31 denotes SP where the immediate arithmetic encoding permits it and XZR in the listed zero-register forms. ADDS/SUBS set N, Z, C and V without changing SP. SUBS sets C when there is no unsigned borrow. BL writes the next guest PC to X30, and RET reads its destination register. The next fetch detects a misaligned return destination.

The memory profile orders guest accesses within a serialized execution session. Both backends enforce 16-byte stack-pointer alignment before applying an offset and support the DMB ISH fence. Atomics/exclusives, parallel weak-memory outcomes, vector instructions, privilege and OS services are outside this profile. The baseline profile requires no optional ISA features. [Armv8-A instruction set guide](https://developer.arm.com/-/media/Arm%20Developer%20Community/PDF/Learn%20the%20Architecture/Armv8-A%20Instruction%20Set%20Architecture.pdf)

The ARM64 guest suite compares interpreter and JIT against 912 arithmetic cases and all 256 condition/NZCV combinations, with ARM64 ISA reference instructions when available. Raw fixtures also check memory, DMB, BL/RET, SP, XZR, and precise fault PCs. See [portability](portability.md) for the host matrix and [contributing](../CONTRIBUTING.md) for cross-test commands.
