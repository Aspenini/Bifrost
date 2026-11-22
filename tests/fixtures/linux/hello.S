/* Self-authored Linux x86-64 fixture. No libc or loader. */
.global _start
.text
_start:
    movq $1, %rax
    movq $1, %rdi
    movq $message, %rsi
    movq $message_end - message, %rdx
    syscall
    movq $60, %rax
    movq $42, %rdi
    syscall
.section .rodata
message:
    .ascii "Hello from Bifrost ELF!\n"
message_end:
.bss
.space 8
.section .note.GNU-stack,"",@progbits
