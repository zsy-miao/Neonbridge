"""Independent RDNA2 ordinary packed integer model.

Two independently selected 16-bit halves, no special registers or modifiers
other than CLAMP on add/subtract/multiply/MAD. Model uses arbitrary precision
Python arithmetic, not the HLSL expressions or DXIL output.
"""
FORMS = {
    "v_pk_mad_i16": 3, "v_pk_mul_lo_u16": 2, "v_pk_add_i16": 2,
    "v_pk_sub_i16": 2, "v_pk_lshlrev_b16": 2, "v_pk_lshrrev_b16": 2,
    "v_pk_ashrrev_i16": 2, "v_pk_max_i16": 2, "v_pk_min_i16": 2,
    "v_pk_mad_u16": 3, "v_pk_add_u16": 2, "v_pk_sub_u16": 2,
    "v_pk_max_u16": 2, "v_pk_min_u16": 2,
}
SINGLE = {
    "v_add_nc_u16": "v_pk_add_u16", "v_sub_nc_u16": "v_pk_sub_u16",
    "v_mul_lo_u16": "v_pk_mul_lo_u16",
    "v_max_u16": "v_pk_max_u16", "v_min_u16": "v_pk_min_u16",
    "v_max_i16": "v_pk_max_i16", "v_min_i16": "v_pk_min_i16",
    "v_lshlrev_b16": "v_pk_lshlrev_b16", "v_lshrrev_b16": "v_pk_lshrrev_b16",
    "v_ashrrev_i16": "v_pk_ashrrev_i16",
}
CLAMP = frozenset({
    "v_pk_mad_i16", "v_pk_mad_u16", "v_pk_add_i16", "v_pk_add_u16",
    "v_pk_sub_i16", "v_pk_sub_u16", "v_pk_mul_lo_u16",
})
SAT = ("v_sat_pk_u8_i16_e32", "v_sat_pk_u8_i16_e64")
MASK = 0xffff
REGS = tuple(range(4, 10))

def signed(value):
    return value - 65536 if value >= 32768 else value

def result(name, values, low, high, saturate=False):
    n = FORMS[name]
    if len(values) != n or not all(type(x) is int and 0 <= x <= 0xffffffff for x in values):
        raise ValueError("invalid source words")
    if not 0 <= low < 2**n or not 0 <= high < 2**n or saturate and name not in CLAMP:
        raise ValueError("unsupported selectors or clamp")
    def half(selector):
        a,b,*rest = [(v >> (16*((selector >> i)&1))) % 65536 for i,v in enumerate(values)]
        if name.endswith("_i16") and not "ashr" in name:
            a,b,*rest = map(signed, [a,b,*rest])
        if "_mad_" in name:
            x = a*b+rest[0]
        elif "_add_" in name:
            x = a+b
        elif "_sub_" in name:
            x = a-b
        elif "_mul_" in name:
            x = a*b
        elif "_lshlrev_" in name:
            x = b * 2**(a%16)
        elif "_lshrrev_" in name:
            x = b // 2**(a%16)
        elif "_ashrrev_" in name:
            x = signed(b) // 2**(a%16)
        elif "_max_" in name:
            x = max(a,b)
        elif "_min_" in name:
            x = min(a,b)
        else:
            raise ValueError(name)
        if saturate:
            lower,upper = (-32768,32767) if name.endswith("_i16") else (0,65535)
            x = min(upper,max(lower,x))
        return x % 65536
    return half(low) + 65536*half(high)

def execute(initial, instructions):
    if len(initial)!=len(REGS) or any(type(v) is not int or not 0<=v<=0xffffffff for v in initial):
        raise ValueError("invalid initial registers")
    state=dict(zip(REGS,initial))
    for name,dest,sources,lo,hi,clamp in instructions:
        if dest not in state or any(s not in state for s in sources):
            raise ValueError("only initialized ordinary VGPRs")
        values=[state[s] for s in sources]
        if name in SAT:
            if len(values) != 1 or lo or hi or clamp:
                raise ValueError("unsupported saturating pack modifiers")
            # Independent signed-half arithmetic: each signed half maps to
            # an unsigned byte, with the high destination word cleared.
            halves = [signed((values[0] // (65536**i)) % 65536) for i in (0,1)]
            state[dest] = sum(min(255,max(0,value)) * 256**i for i,value in enumerate(halves))
        elif name in SINGLE:
            if clamp or hi not in (0,1):
                raise ValueError("unsupported single-half modifier")
            value=result(SINGLE[name],values,lo,lo)&MASK
            shift=16*hi
            state[dest]=(state[dest]&~(MASK<<shift))|(value<<shift)
        else:
            state[dest]=result(name,values,lo,hi,clamp)
    return [state[r] for r in REGS]
