.intel_syntax noprefix
.text
.global _start
_start:
    sub rsp, 8
    mov qword ptr [rsp], 41
    mov rax, 24
    syscall
    mov rax, [rsp]
    cmp rax, 41
    jne failed
    mov rax, [rip + shared]
    cmp rax, 2
    jne failed
    mov rdi, 42
    mov rax, 60
    syscall
failed:
    mov rdi, 99
    mov rax, 231
    syscall

.org 128, 0x90
worker:
    sub rsp, 8
    mov [rsp], rdi
    mov rax, 186
    syscall
    mov [rip + shared], rax
    mov rax, 24
    syscall
    mov rdi, [rsp]
    mov rax, 60
    syscall

.org 256, 0x90
terminate_group:
    mov rax, 231
    syscall

.org 288, 0x90
fault:
    ud2

.org 320, 0x90
spin:
    jmp spin

.data
.balign 8
shared:
    .quad 0
