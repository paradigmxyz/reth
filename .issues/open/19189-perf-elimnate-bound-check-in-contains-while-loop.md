---
title: 'perf: elimnate bound check in `contains` `while` loop'
labels:
    - A-trie
    - C-perf
assignees:
    - gancerlory
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.001136Z
info:
    author: malik672
    created_at: 2025-10-21T15:25:02Z
    updated_at: 2025-10-22T07:52:09Z
---

```
   pub fn contains(&mut self, prefix: &Nibbles) -> bool {
        if self.all {
            return true
        }

        while self.index > 0 && &self.keys[self.index] > prefix {
            self.index -= 1;
        }

        for (idx, key) in self.keys[self.index..].iter().enumerate() {
            if key.starts_with(prefix) {
                self.index += idx;
                return true
            }

            if key > prefix {
                self.index += idx;
                return false
            }
        }

        false
    }

```
currently in `PrefixSet::contains` in while loop, a bound check is done on every iteration when the loop is entered

```
LBB0_5:
	sub	x16, x16, #40
	sub	x8, x8, #1
	str	x8, [x0, #8]
	cbz	x8, LBB0_12
LBB0_6:
	cmp	x8, x9
	b.hs	LBB0_34
	ldr	x12, [x11, #24]
	mul	x2, x8, x15
	ldr	x2, [x12, x2]
	sub	x3, x2, x2, lsr #1
	cmp	x14, x3
	csel	x3, x14, x3, lo
	cmp	x3, #32
	b.hi	LBB0_32
	mov	x4, x16
	mov	x5, x13
LBB0_9:
	cbz	x3, LBB0_4
	sub	x3, x3, #1
	ldrb	w6, [x12, x4]
	ldrb	w7, [x5], #-1
	sub	x4, x4, #1
	cmp	w6, w7
	b.eq	LBB0_9
	b.hi	LBB0_5
```

`while` loop is entered at:
```
LBB0_6:
	cmp	x8, x9
	b.hs	LBB0_34
```

Placing an check at the top removes the bound check, something like:
```
   if self.index >= self.keys.len() {
            self.index = 0;
        }
```

and we get seomthing like this: 
```
    mul     x2, x8, x15       
    ldr     x2, [x11, x2]       

```



