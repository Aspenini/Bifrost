.intel_syntax noprefix
.section .text
.global _start
_start:
    mov rsi, qword ptr [rip + message_pointer]
    mov rax, 1
    mov rdi, 1
    mov rdx, 24
    syscall
    mov rax, 60
    mov rdi, 44
    syscall

.section .rodata
message:
    .ascii "Hello from Bifrost PIE!\n"

.section .data.rel.ro
.balign 8
message_pointer:
    .quad message
