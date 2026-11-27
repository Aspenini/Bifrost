# ARM64 scalar guest profile

The raw profile uses little-endian A64 instructions and serialized execution over owned normal guest memory. Every instruction fetch requires four-byte PC alignment and execute permission. The decoder preserves the instruction word and PC. It reports words outside the listed profile as unsupported. UDF produces a guest illegal-instruction fault.

| Family | Accepted forms |
| --- | --- |
| MOV | MOVZ, MOVN and MOVK to W or X registers, and MOV from a W or X register via the unshifted ORR alias |
| Arithmetic | 32-bit and 64-bit ADD, ADDS, SUB and SUBS with imm12 (optional LSL #12) or register LSL, LSR and ASR, including CMP/CMN and immediate moves to/from WSP or SP |
| Multiply | 32-bit and 64-bit MADD and MSUB, including MUL and MNEG |
| Address generation | ADR and ADRP |
| Logical | 32-bit and 64-bit AND, ORR, EOR and ANDS with a bitmask immediate or shifted register, plus shifted-register BIC, ORN, EON and BICS |
| Bitfield | 32-bit and 64-bit BFM, SBFM and UBFM, including immediate shift and sign-extension aliases. EXTR includes the immediate ROR alias. |
| Bit operations | 32-bit and 64-bit RBIT, CLZ, CLS, REV, and REV16, plus REV32 on 64-bit registers. |
| Checksums | CRC32 and CRC32C with byte, halfword, word, and doubleword input. |
| Variable shifts | 32-bit and 64-bit LSLV, LSRV, ASRV and RORV |
| Conditional select | 32-bit and 64-bit CSEL, CSINC, CSINV and CSNEG |
| Conditional compare | 32-bit and 64-bit CCMP and CCMN with a register or immediate |
| Memory | 32-bit and 64-bit unsigned scaled immediate and signed pre/post-indexed LDR/STR, signed unscaled immediate LDUR/STUR, unsigned immediate LDRB/STRB, and signed unscaled immediate LDURB/STURB with Xn or SP base |
| Register-offset memory | Unsigned byte, halfword, word and doubleword loads and stores, plus LDRSB, LDRSH and LDRSW, with Xn or SP base and UXTW, SXTW, SXTX or LSL offset. |
| Paired memory | 32-bit and 64-bit general-purpose pairs and 32-bit, 64-bit and 128-bit SIMD pairs. Supports signed-offset, pre-index and post-index forms with an Xn or SP base and distinct LDP destinations. |
| Atomic memory | LDADD, LDCLR, LDSET, and SWP for 8-, 16-, 32-, and 64-bit values, including acquire and release forms and zero-register destinations. |
| SIMD scalar and vector transfers | B, H, S, D and Q loads and stores with scaled or unscaled immediate offsets, including pre-index and post-index writeback. FMOV supports S/D immediates, same-width S/D register moves, and bit transfers between W/S or X/D registers. |
| Scalar floating point | Binary32 and binary64 fused multiply-add/subtract (FMADD, FMSUB, FNMADD, FNMSUB), FADD, FSUB, FMUL, FDIV, FCMP, FCMPE, FCCMP and FCCMPE, and unscaled SCVTF/UCVTF from W/X registers. The raw runtime shares an exact rounding path across both backends, with cumulative FPSR flags, FPCR rounding modes, default-NaN, and flush-to-zero support. Other FPCR controls are reported as unsupported. |
| SIMD immediates | Integer MOVI, including byte masks in 64-bit lanes. |
| SIMD lanes | Single-register LD1/ST1 for one B/H/S/D lane, including immediate and register post-index forms. |
| Floating-point system registers | MRS and MSR for FPCR and FPSR in the base AArch64 profile. Guest writes ignore reserved bits. |
| Floating-point precision and unary operations | Scalar FSQRT and FCVT between binary32 and binary64. FABS and FNEG for scalars and 2S/4S/2D vectors in the supported FPCR profile. |
| Integer and floating-point conversions | Unscaled SCVTF, UCVTF, FCVTZS, and FCVTZU with binary32 or binary64 and W/X registers or SIMD S/D, 2S/4S/2D lanes. Float-to-integer conversions saturate and update cumulative exception flags. |
| Branch | B, BL, B.cond, BR, BLR, 32-bit and 64-bit CBZ/CBNZ, TBZ/TBNZ, RET Xn |
| Miscellaneous | NOP, DMB ISH, DSB ISH, DSB SY, ISB SY, UDF. ISB uses the raw runtime's instruction boundary and translated-code generation checks. |
| Supervisor call | SVC with a 16-bit immediate, reported to the caller as an execution exit |
| Pointer authentication | Register PAC/AUT, PACIASP/PACIBSP, AUTIASP/AUTIBSP, RETAA/RETAB and XPACI/XPACD/XPACLRI produce typed host exits before modifying guest state |

Register 31 denotes SP where the immediate arithmetic encoding permits it and XZR in the listed zero-register forms. ADDS/SUBS set N, Z, C and V without changing SP. SUBS sets C when there is no unsigned borrow. BL writes the next guest PC to X30, and RET reads its destination register. The next fetch detects a misaligned return destination.

The memory profile orders guest accesses within a serialized execution session. Both backends enforce 16-byte stack-pointer alignment before applying an offset and support the DMB ISH fence. SIMD register pairs check the full memory range before updating registers or committing a store. The baseline profile requires no optional ISA features. [Armv8-A instruction set guide](https://developer.arm.com/-/media/Arm%20Developer%20Community/PDF/Learn%20the%20Architecture/Armv8-A%20Instruction%20Set%20Architecture.pdf)

The ARM64 guest suite compares interpreter and JIT against 912 arithmetic cases and all 256 condition/NZCV combinations, with ARM64 ISA reference instructions when available. Raw fixtures also check memory, DMB, BL/RET, SP, XZR, and precise fault PCs. See [portability](portability.md) for the host matrix and [contributing](../CONTRIBUTING.md) for cross-test commands.
