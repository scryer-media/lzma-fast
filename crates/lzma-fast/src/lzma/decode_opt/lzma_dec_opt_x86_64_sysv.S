// Port of Asm/x86/LzmaDecOpt.asm from Igor Pavlov's LZMA SDK 26.03 (commit
// 0766b733), public domain, with Asm/x86/7zAsm.asm spliced in. This is the
// body of the `Z7_LZMA_DEC_OPT` replacement for LzmaDec_DecodeReal_3 for the
// System V calling convention, included into a Rust naked function by
// src/lzma/decode_opt/x86_64.rs.
//
// The reference file is MASM; this is GNU Intel syntax, which is what Rust's
// `naked_asm!` hands to LLVM's integrated assembler. Everything MASM does
// that GNU as does not was resolved by hand, and nothing else was changed:
//
//   * macros are fully expanded, and `equ` (which is textual in MASM, so it
//     names registers as well as constants) is substituted;
//   * every constant expression is folded to a plain integer, which is not
//     cosmetic: LLVM's GNU-Intel parser silently drops the index register of
//     a memory operand whose scale is parenthesised, so `[probs + sym * (2)]`
//     would assemble as `[probs]`;
//   * conditional assembly is resolved for x64 with ABI_LINUX defined, matching the
//     reference file's own configuration (_LZMA_PROB32 and _LZMA_SIZE_OPT
//     off, Z7_LZMA_DEC_OPT_ASM_USE_SEGMENT irrelevant without a SEGMENT);
//   * MASM `struct` field references (`[r1].CLzmaDec_Asm.rep0`) became
//     explicit displacements with an explicit operand-size qualifier, since
//     GNU as cannot infer the size from a field type;
//   * `@@` / `@f` / `@b` anonymous labels became numbered ones, `@CatStr`
//     was pasted, `SHL`/`SHR` became `<<`/`>>`, `100h` became `0x100`,
//     `SHORT` and `near ptr` were dropped, and every label gained an `L`
//     prefix because Mach-O wants assembler-local labels;
//   * `MY_PROC` / `MY_ENDP` / `SEGMENT` are gone; the Rust naked function
//     supplies the symbol, and the reference epilogue's `ret` is kept.
//
// The register allocation comments, the structure offsets and the code are
// the reference file's.

	.p2align 4
    push    RBX
    push    RBP
        push    r12
        push    r13
        push    r14
        push    r15
        lea     RAX, [RSP - 128]
        and     RAX, -128
        mov     RBP, RSP
        mov     RSP, RAX
        mov     qword ptr [RAX], RBP
        mov     qword ptr [RAX + 8], RDI
        mov     dword ptr [RAX + 100], 0
        mov     qword ptr [RAX + 72], RDX
        mov     RBX, RDI
        mov     r12, qword ptr [RBX + 24]
        add     RSI, r12
        mov     qword ptr [RAX + 64], RSI
        mov     EDI, dword ptr [RBX + 72]
        mov     dword ptr [RAX + 112], EDI
        mov     EDI, dword ptr [RBX + 76]
        mov     dword ptr [RAX + 116], EDI
        mov     EDI, dword ptr [RBX + 80]
        mov     dword ptr [RAX + 120], EDI
        mov     EDI, dword ptr [RBX + 84]
        mov     dword ptr [RAX + 124], EDI
        mov     r14, qword ptr [RBX + 40]
        add     r14, r12
        mov     qword ptr [RAX + 104], r14
        mov     qword ptr [RAX + 56], r12
        mov     CL, byte ptr [RBX + 2]
        mov     EDI, 1
        shl     EDI, CL
        dec     EDI
        mov     dword ptr [RAX + 88], EDI
        mov     CL, byte ptr [RBX]
        mov     EDX, 0x100
        mov     EDI, EDX
        shr     EDX, CL
        add     CL, 1
        mov     dword ptr [RAX + 80], ECX
        mov     CL, byte ptr [RBX + 1]
        shl     EDI, CL
        sub     EDI, EDX
        mov     dword ptr [RAX + 84], EDI
        mov     r9d, EDI
        mov     r11, qword ptr [RBX + 16]
        mov     qword ptr [RAX + 48], r11
        mov     RDI, qword ptr [RBX + 32]
        mov     qword ptr [RAX + 40], RDI
        mov     ECX, dword ptr [RBX + 68]
        mov     dword ptr [RAX + 92], ECX
        mov     r13d, dword ptr [RBX + 64]
        mov     r8d, dword ptr [RBX + 88]
        shl     r8d, 1
        mov     r15,   qword ptr [RBX + 48]
        mov     EAX, dword ptr [RBX + 56]
        mov     EBP,   dword ptr [RBX + 60]
        mov     r10d, 2048
        xor     EBX, EBX
        or      ECX, r13d
        jz      Lanon0
        add     RDI, r12
        cmp     r14, r12
        cmovnz  RDI, r14
        movzx   EBX, byte ptr[RDI - 1]
Lanon0:
        mov     ECX, dword ptr [RSP + 88]
        and     ECX, r13d
        shl     ECX, 5
        lea     RSI, [r11 + 1 * r8]
        cmp     r8d, 8
        jb      Llit_end
        cmp     r8d, 14
        jb      Llit_matched_end
        jmp     Llz_end
	.p2align 6
Llit_start:
        xor     r8d, r8d
Llit_start_2:
        mov     EDI, r13d
        shl     EDI, 8
        add     EBX, EDI
        and     EBX, r9d
        add     RSI, RCX
        mov     ECX, dword ptr [RSP + 80]
        lea     EBX, dword ptr[RBX + 2 * RBX]
        add     r11, 640
        shl     EBX, CL
        add     r11, RBX
        mov     EDI, r10d
        sub     EDI, EDX
        shr     EDI, 5
        add     EDX, EDI
                mov     word ptr [0 + RSI + -512], DX
        inc     r13d
                movzx   ECX, word ptr [r11 + 2]
                movzx   EDX, word ptr [r11 + 4]
        cmp     EAX, 16777216
        jae     Lanon1
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon1:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + 4 + 2]
        cmovae  EDX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        cmovb   EDI, r10d
        mov     EBX, 2
        sbb     EBX, 0 - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [r11 + 2], DI
                movzx   ECX, word ptr [r11 + RBX * 4]
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon2
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon2:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + RBX * 2 + 2]
        cmovae  ECX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 0 - 1
        sub     EDI, EDX
        sar     EDI, 5
        add     EDI, EDX
                mov     word ptr [r11 + RSI * 1], DI
                movzx   EDX, word ptr [r11 + RBX * 4]
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon3
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon3:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + RBX * 2 + 2]
        cmovae  EDX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 0 - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [r11 + RSI * 1], DI
                movzx   ECX, word ptr [r11 + RBX * 4]
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon4
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon4:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + RBX * 2 + 2]
        cmovae  ECX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 0 - 1
        sub     EDI, EDX
        sar     EDI, 5
        add     EDI, EDX
                mov     word ptr [r11 + RSI * 1], DI
                movzx   EDX, word ptr [r11 + RBX * 4]
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon5
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon5:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + RBX * 2 + 2]
        cmovae  EDX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 0 - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [r11 + RSI * 1], DI
                movzx   ECX, word ptr [r11 + RBX * 4]
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon6
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon6:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + RBX * 2 + 2]
        cmovae  ECX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 0 - 1
        sub     EDI, EDX
        sar     EDI, 5
        add     EDI, EDX
                mov     word ptr [r11 + RSI * 1], DI
                movzx   EDX, word ptr [r11 + RBX * 4]
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon7
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon7:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + RBX * 2 + 2]
        cmovae  EDX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 0 - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [r11 + RSI * 1], DI
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon8
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon8:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 256 - 1
        sub     EDI, EDX
        sar     EDI, 5
        add     EDI, EDX
                mov     word ptr [r11 + RSI * 1], DI
        mov     r11, qword ptr [RSP + 48]
        mov     ECX, dword ptr [RSP + 88]
        and     ECX, r13d
        shl     ECX, 5
        lea     RSI, [r11 + 1 * r8]
        mov     byte ptr[r14], BL
        inc     r14
        cmp     r15, qword ptr [RSP + 72]
        jae     Lfin_OK
        cmp     r14, qword ptr [RSP + 64]
        jae     Lfin_OK
Llit_end:
                movzx   EDX, word ptr [RCX * 1 + RSI + -512]
        cmp     EAX, 16777216
        jae     Lanon9
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon9:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        cmp     EBP, EAX
        jb      Llit_start
LIsMatch_label:
        sub     EDI, EAX
        sub     EBP, EAX
        mov     EAX, EDI
        mov     EDI, EDX
        shr     EDX, 5
        sub     EDI, EDX
                mov     word ptr [RCX * 1 + RSI + -512], DI
                movzx   EDX, word ptr [0 + RSI + 32]
        cmp     EAX, 16777216
        jae     Lanon10
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon10:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        cmp     EBP, EAX
        jae     LIsRep_label
        mov     EDI, r10d
        sub     EDI, EDX
        shr     EDI, 5
        add     EDX, EDI
                mov     word ptr [0 + RSI + 32], DX
        add     r11, -1536
        add     r8d, 24
Llen_decode:
        mov     r12d, 8 - 1 - 2
                movzx   EDX, word ptr [0 + r11 + 0]
        cmp     EAX, 16777216
        jae     Lanon11
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon11:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        cmp     EBP, EAX
        jb      Llen_mid_0
        sub     EDI, EAX
        sub     EBP, EAX
        mov     EAX, EDI
        mov     EDI, EDX
        shr     EDX, 5
        sub     EDI, EDX
                mov     word ptr [0 + r11 + 0], DI
        add     r11, 16
        mov     r12d, -1 - 2
                movzx   EDX, word ptr [0 + r11 + 0]
        cmp     EAX, 16777216
        jae     Lanon12
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon12:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        cmp     EBP, EAX
        jb      Llen_mid_0
        sub     EDI, EAX
        sub     EBP, EAX
        mov     EAX, EDI
        mov     EDI, EDX
        shr     EDX, 5
        sub     EDI, EDX
                mov     word ptr [0 + r11 + 0], DI
        add     r11, 512 - 16
        mov     EBX, 1
                movzx   ECX, word ptr [r11 + 2]
	.p2align 5
Llen8_loop:
                movzx   EDX, word ptr [r11 + RBX * 4]
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon13
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon13:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + RBX * 2 + 2]
        cmovae  EDX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 0 - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [r11 + RSI * 1], DI
        mov     ECX, EDX
        cmp     EBX, 64
        jb      Llen8_loop
        mov     r12d, 240 - 1 - 2
        jmp     Llen_mid_2
	.p2align 5
Llen_mid_0:
        mov     EDI, r10d
        sub     EDI, EDX
        shr     EDI, 5
        add     EDX, EDI
                mov     word ptr [0 + r11 + 0], DX
        add     r11, RCX
                movzx   EDX, word ptr [r11 + 2]
                movzx   ECX, word ptr [r11 + 4]
        cmp     EAX, 16777216
        jae     Lanon14
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon14:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + 4 + 2]
        cmovae  ECX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        cmovb   EDI, r10d
        mov     EBX, 2
        sbb     EBX, 0 - 1
        sub     EDI, EDX
        sar     EDI, 5
        add     EDI, EDX
                mov     word ptr [r11 + 2], DI
Llen_mid_2:
                movzx   EDX, word ptr [r11 + RBX * 4]
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon15
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon15:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + RBX * 2 + 2]
        cmovae  EDX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 0 - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [r11 + RSI * 1], DI
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon16
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon16:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, r12d
        sub     EDI, EDX
        sar     EDI, 5
        add     EDI, EDX
                mov     word ptr [r11 + RSI * 1], DI
        mov     r11, qword ptr [RSP + 48]
        cmp     r8d, 24
        jb      Lcopy_match
        mov     EDI, 3 + 2
        cmp     EBX, 3 + 2
        cmovb   EDI, EBX
        add     r11, 128 - 256
        shl     EDI, 7
        add     r11, RDI
        mov     r12d, EBX
                movzx   ECX, word ptr [r11 + 2]
                movzx   EDX, word ptr [r11 + 4]
        cmp     EAX, 16777216
        jae     Lanon17
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon17:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + 4 + 2]
        cmovae  EDX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        cmovb   EDI, r10d
        mov     EBX, 2
        sbb     EBX, 0 - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [r11 + 2], DI
                movzx   ECX, word ptr [r11 + RBX * 4]
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon18
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon18:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + RBX * 2 + 2]
        cmovae  ECX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 0 - 1
        sub     EDI, EDX
        sar     EDI, 5
        add     EDI, EDX
                mov     word ptr [r11 + RSI * 1], DI
                movzx   EDX, word ptr [r11 + RBX * 4]
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon19
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon19:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + RBX * 2 + 2]
        cmovae  EDX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 0 - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [r11 + RSI * 1], DI
                movzx   ECX, word ptr [r11 + RBX * 4]
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon20
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon20:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + RBX * 2 + 2]
        cmovae  ECX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 0 - 1
        sub     EDI, EDX
        sar     EDI, 5
        add     EDI, EDX
                mov     word ptr [r11 + RSI * 1], DI
                movzx   EDX, word ptr [r11 + RBX * 4]
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon21
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon21:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + RBX * 2 + 2]
        cmovae  EDX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 0 - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [r11 + RSI * 1], DI
        mov     ECX, EBX
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon22
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon22:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 64-1
        sub     EDI, EDX
        sar     EDI, 5
        add     EDI, EDX
                mov     word ptr [r11 + RSI * 1], DI
        and     EBX, 3
        mov     r11, qword ptr [RSP + 48]
        cmp     ECX, 32 + 14 / 2
        jb      Lshort_dist
        sub     ECX, 37
        or      EBX, 2
                movzx   EDX, word ptr [r11 + 2]
        shl     EBX, 4 + 1
        lea     r9, [r11 + 4]
        jmp     Ldirect_norm
	.p2align 5
Ldirect_loop:
        shr     EAX, 1
        mov     EDI, EBP
        sub     EBP, EAX
        cmovs   EBP, EDI
        cmovns  EBX, ESI
        dec     ECX
        je      Ldirect_end
        add     EBX, EBX
Ldirect_norm:
        lea     ESI, [RBX + 16]
        cmp     EAX, 16777216
        jae     Ldirect_loop
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
        jmp     Ldirect_loop
	.p2align 5
Ldirect_end:
                movzx   ECX, word ptr [r9]
        cmp     EAX, 16777216
        jae     Lanon23
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon23:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r11 + 6]
        cmovae  ECX, EDI
        cmovb   EBP, ESI
        mov     EDI, 31
        cmovb   EDI, r10d
        lea     RSI, [r11 + 6]
        cmovae  r9, RSI
        sub     EDI, EDX
        sar     EDI, 5
        add     EDI, EDX
                mov     word ptr [r11 + 2], DI
        add     r9, 4
                movzx   EDX, word ptr [r9]
        cmp     EAX, 16777216
        jae     Lanon24
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon24:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r9 + 4]
        cmovae  EDX, EDI
        cmovb   EBP, ESI
        mov     EDI, 31
        cmovb   EDI, r10d
        lea     RSI, [r9 + 4]
        cmovae  r9, RSI
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [RSI - 8], DI
        add     r9, 8
                movzx   ECX, word ptr [r9]
        cmp     EAX, 16777216
        jae     Lanon25
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon25:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
                movzx   EDI, word ptr [r9 + 8]
        cmovae  ECX, EDI
        cmovb   EBP, ESI
        mov     EDI, 31
        cmovb   EDI, r10d
        lea     RSI, [r9 + 8]
        cmovae  r9, RSI
        sub     EDI, EDX
        sar     EDI, 5
        add     EDI, EDX
                mov     word ptr [RSI - 16], DI
        sub     r9, r11
        shr     r9d, 1
        or      EBX, r9d
        cmp     EAX, 16777216
        jae     Lanon26
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon26:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
        lea     EDI, [EBX - 8]
        cmovb   EBX, EDI
        cmovb   EBP, ESI
        mov     EDI, 31
        cmovb   EDI, r10d
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [r11 + r9 * 2], DI
Ldecode_dist_end:
        mov     ESI, dword ptr [RSP + 112]
        mov     ECX, dword ptr [RSP + 116]
        mov     EDX, dword ptr [RSP + 120]
        mov     EDI, dword ptr [RSP + 92]
        test    EDI, EDI
        cmove   EDI, r13d
        cmp     EBX, EDI
        jae     Lend_of_payload
        inc     EBX
        mov     dword ptr [RSP + 112], EBX
        mov     EBX, r12d
        mov     dword ptr [RSP + 116], ESI
        mov     dword ptr [RSP + 120], ECX
        mov     dword ptr [RSP + 124], EDX
        cmp     r8d, 38
        mov     r8d, 14
        mov     EDI, 20
        cmovae  r8d, EDI
Lcopy_match:
        mov     RDX, qword ptr [RSP + 64]
        sub     RDX, r14
        jz      Lfin_dicPos_LIMIT
        cmp     RDX, RBX
        cmovae  EDX, EBX
        mov     r12, qword ptr [RSP + 56]
        mov     ECX, dword ptr [RSP + 112]
        mov     RDI, r14
        add     r14, RDX
        add     r13d, EDX
        sub     EBX, EDX
        mov     dword ptr [RSP + 100], EBX
        sub     RDI, r12
        sub     RDI, RCX
        jae     Lanon27
        mov     RCX, qword ptr [RSP + 40]
        add     RDI, RCX
        sub     RCX, RDI
        cmp     RDX, RCX
        ja      Lcopy_match_cross
Lanon27:
        add     RDI, r12
        movzx   EBX, byte ptr[RDI]
        add     RDI, RDX
        neg     RDX
Lcopy_common:
        dec     r14
        mov     ECX, dword ptr [RSP + 88]
        and     ECX, r13d
        shl     ECX, 5
        lea     RSI, [r11 + 1 * r8]
        inc     RDX
        jz      Lcopy_end
	.p2align 4
Lanon28:
        mov     byte ptr[RDX * 1 + r14], BL
        movzx   EBX, byte ptr[RDX * 1 + RDI]
        inc     RDX
        jnz     Lanon28
Lcopy_end:
Llz_end_match:
        mov     byte ptr[r14], BL
        inc     r14
        cmp     r15, qword ptr [RSP + 72]
        jae     Lfin_OK
        cmp     r14, qword ptr [RSP + 64]
        jae     Lfin_OK
Llz_end:
                movzx   EDX, word ptr [RCX * 1 + RSI + -512]
        cmp     EAX, 16777216
        jae     Lanon29
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon29:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        cmp     EBP, EAX
        jae     LIsMatch_label
        mov     EDI, r13d
        shl     EDI, 8
        add     EBX, EDI
        and     EBX, dword ptr [RSP + 84]
        add     RSI, RCX
        mov     ECX, dword ptr [RSP + 80]
        lea     EBX, dword ptr[RBX + 2 * RBX]
        add     r11, 640
        shl     EBX, CL
        add     r11, RBX
        mov     EDI, r10d
        sub     EDI, EDX
        shr     EDI, 5
        add     EDX, EDI
                mov     word ptr [0 + RSI + -512], DX
        inc     r13d
        mov     ECX, dword ptr [RSP + 112]
        mov     qword ptr [RSP + 104], r14
        lea     EDI, [r8 - 12]
        sub     r8d, 6
        cmp     r8d, 14
        cmovae  r8d, EDI
        sub     r14, r12
        sub     r14, RCX
        jae     Lanon30
        add     r14, qword ptr [RSP + 40]
Lanon30:
        movzx   r9d, byte ptr[r12 + r14 * 1]
        mov     r12d, 512
        shl     r9d, 2
        mov     r14d, r12d
        and     r14d, r9d
                movzx   ECX, word ptr [r11 + 512 + r14 * 1 + 2]
        lea     RDX, [r11 + 512 + r14 * 1 + 2]
        xor     r12d, r14d
        add     r9d, r9d
        cmp     EAX, 16777216
        jae     Lanon31
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon31:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  r12d, r14d
        mov     r14d, r9d
        cmovae  EAX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        cmovb   EDI, r10d
        mov     EBX, 0
        sbb     EBX, -2-1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [RDX], DI
        and     r14d, r12d
        lea     RDX, [r11 + r12 * 1]
        add     RDX, r14
                movzx   ECX, word ptr [RDX + RBX * 2]
        xor     r12d, r14d
        add     EBX, EBX
        add     r9d, r9d
        cmp     EAX, 16777216
        jae     Lanon32
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon32:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  r12d, r14d
        mov     r14d, r9d
        cmovae  EAX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [RDX + RSI * 1], DI
        and     r14d, r12d
        lea     RDX, [r11 + r12 * 1]
        add     RDX, r14
                movzx   ECX, word ptr [RDX + RBX * 2]
        xor     r12d, r14d
        add     EBX, EBX
        add     r9d, r9d
        cmp     EAX, 16777216
        jae     Lanon33
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon33:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  r12d, r14d
        mov     r14d, r9d
        cmovae  EAX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [RDX + RSI * 1], DI
        and     r14d, r12d
        lea     RDX, [r11 + r12 * 1]
        add     RDX, r14
                movzx   ECX, word ptr [RDX + RBX * 2]
        xor     r12d, r14d
        add     EBX, EBX
        add     r9d, r9d
        cmp     EAX, 16777216
        jae     Lanon34
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon34:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  r12d, r14d
        mov     r14d, r9d
        cmovae  EAX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [RDX + RSI * 1], DI
        and     r14d, r12d
        lea     RDX, [r11 + r12 * 1]
        add     RDX, r14
                movzx   ECX, word ptr [RDX + RBX * 2]
        xor     r12d, r14d
        add     EBX, EBX
        add     r9d, r9d
        cmp     EAX, 16777216
        jae     Lanon35
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon35:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  r12d, r14d
        mov     r14d, r9d
        cmovae  EAX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [RDX + RSI * 1], DI
        and     r14d, r12d
        lea     RDX, [r11 + r12 * 1]
        add     RDX, r14
                movzx   ECX, word ptr [RDX + RBX * 2]
        xor     r12d, r14d
        add     EBX, EBX
        add     r9d, r9d
        cmp     EAX, 16777216
        jae     Lanon36
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon36:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  r12d, r14d
        mov     r14d, r9d
        cmovae  EAX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [RDX + RSI * 1], DI
        and     r14d, r12d
        lea     RDX, [r11 + r12 * 1]
        add     RDX, r14
                movzx   ECX, word ptr [RDX + RBX * 2]
        xor     r12d, r14d
        add     EBX, EBX
        add     r9d, r9d
        cmp     EAX, 16777216
        jae     Lanon37
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon37:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  r12d, r14d
        mov     r14d, r9d
        cmovae  EAX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [RDX + RSI * 1], DI
        and     r14d, r12d
        lea     RDX, [r11 + r12 * 1]
        add     RDX, r14
                movzx   ECX, word ptr [RDX + RBX * 2]
        add     EBX, EBX
        cmp     EAX, 16777216
        jae     Lanon38
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon38:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, ECX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
        mov     EDI, 31
        cmovb   EBP, ESI
        mov     ESI, EBX
        cmovb   EDI, r10d
        sbb     EBX, 256 - 1
        sub     EDI, ECX
        sar     EDI, 5
        add     EDI, ECX
                mov     word ptr [RDX + RSI * 1], DI
        mov     r11, qword ptr [RSP + 48]
        mov     ECX, dword ptr [RSP + 88]
        and     ECX, r13d
        shl     ECX, 5
        lea     RSI, [r11 + 1 * r8]
        mov     r14, qword ptr [RSP + 104]
        mov     byte ptr[r14], BL
        inc     r14
        cmp     r15, qword ptr [RSP + 72]
        jae     Lfin_OK
        cmp     r14, qword ptr [RSP + 64]
        jae     Lfin_OK
Llit_matched_end:
                movzx   EDX, word ptr [RCX * 1 + RSI + -512]
        cmp     EAX, 16777216
        jae     Lanon39
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon39:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        cmp     EBP, EAX
        jae     LIsMatch_label
        mov     r9d, dword ptr [RSP + 84]
        sub     r8d, 6
        jmp     Llit_start_2
	.p2align 5
LIsRep0Short_label:
        mov     EDI, r10d
        sub     EDI, EDX
        shr     EDI, 5
        add     EDX, EDI
                mov     word ptr [RCX * 1 + RSI + -3072], DX
        mov     r12, qword ptr [RSP + 56]
        mov     RDI, r14
        mov     EDX, dword ptr [RSP + 112]
        sub     RDI, r12
        sub     r11, -2560
        or      r8d, 2
        inc     r13d
        mov     ECX, dword ptr [RSP + 88]
        and     ECX, r13d
        shl     ECX, 5
        lea     RSI, [r11 + 1 * r8]
        sub     RDI, RDX
        jae     Lanon40
        add     RDI, qword ptr [RSP + 40]
Lanon40:
        movzx   EBX, byte ptr[r12 + RDI * 1]
        jmp     Llz_end_match
	.p2align 5
LIsRep_label:
        sub     EDI, EAX
        sub     EBP, EAX
        mov     EAX, EDI
        mov     EDI, EDX
        shr     EDX, 5
        sub     EDI, EDX
                mov     word ptr [0 + RSI + 32], DI
        cmp     r8d, 14
        mov     r8d, 16
        mov     EDX, 22
        cmovae  r8d, EDX
        add     r11, -2560
                movzx   EDX, word ptr [0 + RSI + 56]
        cmp     EAX, 16777216
        jae     Lanon41
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon41:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        cmp     EBP, EAX
        jae     LIsRepG0_label
        mov     EDI, r10d
        sub     EDI, EDX
        shr     EDI, 5
        add     EDX, EDI
                mov     word ptr [0 + RSI + 56], DX
                movzx   EDX, word ptr [RCX * 1 + RSI + -3072]
        cmp     EAX, 16777216
        jae     Lanon42
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon42:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        cmp     EBP, EAX
        jb      LIsRep0Short_label
        sub     EDI, EAX
        sub     EBP, EAX
        mov     EAX, EDI
        mov     EDI, EDX
        shr     EDX, 5
        sub     EDI, EDX
                mov     word ptr [RCX * 1 + RSI + -3072], DI
        jmp     Llen_decode
	.p2align 5
LIsRepG0_label:
        sub     EDI, EAX
        sub     EBP, EAX
        mov     EAX, EDI
        mov     EDI, EDX
        shr     EDX, 5
        sub     EDI, EDX
                mov     word ptr [0 + RSI + 56], DI
        mov     r9d, dword ptr [RSP + 112]
        mov     EBX, dword ptr [RSP + 116]
        mov     dword ptr [RSP + 116], r9d
                movzx   EDX, word ptr [0 + RSI + 80]
        cmp     EAX, 16777216
        jae     Lanon43
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon43:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        cmp     EBP, EAX
        jae     LIsRepG1_label
        mov     EDI, r10d
        sub     EDI, EDX
        shr     EDI, 5
        add     EDX, EDI
                mov     word ptr [0 + RSI + 80], DX
        mov     dword ptr [RSP + 112], EBX
        jmp     Llen_decode
LIsRepG1_label:
        sub     EDI, EAX
        sub     EBP, EAX
        mov     EAX, EDI
        mov     EDI, EDX
        shr     EDX, 5
        sub     EDI, EDX
                mov     word ptr [0 + RSI + 80], DI
        mov     r9d, dword ptr [RSP + 120]
        mov     dword ptr [RSP + 120], EBX
                movzx   EDX, word ptr [0 + RSI + 104]
        cmp     EAX, 16777216
        jae     Lanon44
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon44:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        cmp     EBP, EAX
        jae     LIsRepG2_label
        mov     EDI, r10d
        sub     EDI, EDX
        shr     EDI, 5
        add     EDX, EDI
                mov     word ptr [0 + RSI + 104], DX
        mov     dword ptr [RSP + 112], r9d
        jmp     Llen_decode
LIsRepG2_label:
        sub     EDI, EAX
        sub     EBP, EAX
        mov     EAX, EDI
        mov     EDI, EDX
        shr     EDX, 5
        sub     EDI, EDX
                mov     word ptr [0 + RSI + 104], DI
        mov     EBX, dword ptr [RSP + 124]
        mov     dword ptr [RSP + 124], r9d
        mov     dword ptr [RSP + 112], EBX
        jmp     Llen_decode
	.p2align 5
Lshort_dist:
        sub     ECX, 32 + 1
        jbe     Ldecode_dist_end
        or      EBX, 2
        shl     EBX, CL
        lea     RBX, [r11 + RBX * 2 + -3328 + 2]
        mov     r9d, 2
	.p2align 5
Lspec_loop:
                movzx   EDX, word ptr [RBX]
        mov     r11, RBX
        add     RBX, r9
        cmp     EAX, 16777216
        jae     Lanon45
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon45:
        mov     EDI, EAX
        shr     EAX, 11
        imul    EAX, EDX
        sub     EDI, EAX
        mov     ESI, EBP
        sub     EBP, EAX
        cmovae  EAX, EDI
        lea     RDI, [RBX + 1 * r9]
        cmovae  RBX, RDI
        mov     EDI, 31
        cmovb   EBP, ESI
        cmovb   EDI, r10d
        add     r9d, r9d
        sub     EDI, EDX
        sar     EDI, 5
        add     EDI, EDX
                mov     word ptr [r11], DI
        dec     ECX
        jnz     Lspec_loop
        mov     r11, qword ptr [RSP + 48]
        sub     EBX, r9d
        sub     EBX, -3328
        sub     RBX, r11
        shr     EBX, 1
        jmp     Ldecode_dist_end
Lcopy_match_cross:
        mov     RSI, RDI
        mov     RDI, r12
        mov     RCX, qword ptr [RSP + 40]
        neg     RDX
Lanon46:
        movzx   EBX, byte ptr[RSI * 1 + RDI]
        inc     RSI
        mov     byte ptr[RDX * 1 + r14], BL
        inc     RDX
        cmp     RSI, RCX
        jne     Lanon46
        movzx   EBX, byte ptr[RDI]
        sub     RDI, RDX
        jmp     Lcopy_common
Lfin_dicPos_LIMIT:
        mov     dword ptr [RSP + 100], EBX
        jmp     Lfin_OK
Lfin_ERROR_MATCH_DIST:
        add     r12d, 512
        mov     dword ptr [RSP + 100], r12d
        mov     dword ptr [RSP + 112], EBX
        mov     dword ptr [RSP + 116], ESI
        mov     dword ptr [RSP + 120], ECX
        mov     dword ptr [RSP + 124], EDX
        cmp     r8d, 38
        mov     r8d, 14
        mov     EDI, 20
        cmovae  r8d, EDI
        mov     EBX, 1
        jmp     Lfin
Lend_of_payload:
        inc     EBX
        jnz     Lfin_ERROR_MATCH_DIST
        mov     dword ptr [RSP + 100], 274
        sub     r8d, 24
Lfin_OK:
        xor     EBX, EBX
Lfin:
        cmp     EAX, 16777216
        jae     Lanon47
        shl     EBP, 8
        mov     BPL, BYTE PTR [r15]
        shl     EAX, 8
        inc     r15
Lanon47:
        mov     RCX, qword ptr [RSP + 8]
        sub     r14, qword ptr [RSP + 56]
        mov     qword ptr [RCX + 40], r14
        mov     qword ptr [RCX + 48], r15
        mov     dword ptr [RCX + 56], EAX
        mov     dword ptr [RCX + 60], EBP
        shr     r8d, 1
        mov     dword ptr [RCX + 88], r8d
        mov     dword ptr [RCX + 64], r13d
        mov     EDI, dword ptr [RSP + 100]
        mov     dword ptr [RCX + 92], EDI
        mov     EDI, dword ptr [RSP + 112]
        mov     dword ptr [RCX + 72], EDI
        mov     EDI, dword ptr [RSP + 116]
        mov     dword ptr [RCX + 76], EDI
        mov     EDI, dword ptr [RSP + 120]
        mov     dword ptr [RCX + 80], EDI
        mov     EDI, dword ptr [RSP + 124]
        mov     dword ptr [RCX + 84], EDI
        mov     EAX, EBX
        mov     RSP, qword ptr [RSP]
        pop     r15
        pop     r14
        pop     r13
        pop     r12
    pop     RBP
    pop     RBX
        ret
