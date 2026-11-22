.intel_syntax noprefix
.globl entry
.text
entry:
    sub rsp, 40
    call qword ptr [rip + __imp_GetTickCount]
    mov rcx, 0
    call qword ptr [rip + __imp_ExitProcess]
