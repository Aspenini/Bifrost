.intel_syntax noprefix
.text
    lea rsi, [rip + message]
    mov rdx, 27
    mov rdi, 1
    mov rax, 0x02000004
    syscall
    mov rdi, 45
    mov rax, 0x02000001
    syscall
message:
    .ascii "Hello from Bifrost Darwin!\n"
