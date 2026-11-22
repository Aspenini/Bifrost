/* Self-authored Windows x64 console fixture. No CRT. */
.intel_syntax noprefix
.globl entry
.text
entry:
    sub rsp, 56
    mov rcx, -11
    call qword ptr [rip + __imp_GetStdHandle]
    mov rcx, rax
    movabs rdx, offset message
    mov r8, 24
    mov r9, rsp
    add r9, 48
    mov qword ptr [rsp + 32], 0
    call qword ptr [rip + __imp_WriteFile]
    mov rcx, 43
    call qword ptr [rip + __imp_ExitProcess]
.section .rdata,"dr"
message:
    .ascii "Hello from Bifrost PE!\r\n"
