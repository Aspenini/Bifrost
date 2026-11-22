.global _start
.text
_start:
    movq $999, %rax
    syscall
.section .note.GNU-stack,"",@progbits
