# MDBX Cursor: How It Works

## What is a Cursor?

A cursor is a **pointer into the B+tree** that tracks your current position and provides
navigation operations. Think of it like a bookmark that can move through sorted data.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           MDBX DATABASE (mmap'd file)                       │
│                                                                             │
│    ┌─────────────────────────────────────────────────────────────────┐      │
│    │                     B+TREE STRUCTURE                            │      │
│    │                                                                 │      │
│    │                        ┌─────────┐                              │      │
│    │                        │ Root    │  ◄── Branch nodes contain    │      │
│    │                        │ [P,S,Z] │      keys + child pointers   │      │
│    │                        └────┬────┘                              │      │
│    │              ┌──────────────┼──────────────┐                    │      │
│    │              ▼              ▼              ▼                    │      │
│    │         ┌─────────┐   ┌─────────┐   ┌─────────┐                 │      │
│    │         │ Branch  │   │ Branch  │   │ Branch  │                 │      │
│    │         │ [A-O]   │   │ [P-R]   │   │ [S-Z]   │                 │      │
│    │         └────┬────┘   └────┬────┘   └────┬────┘                 │      │
│    │              │             │             │                      │      │
│    │    ┌─────────┴───┐   ┌─────┴─────┐   ┌───┴─────────┐            │      │
│    │    ▼             ▼   ▼           ▼   ▼             ▼            │      │
│    │ ┌──────┐     ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐       │      │
│    │ │ Leaf │ ──► │ Leaf │ │ Leaf │ │ Leaf │ │ Leaf │ │ Leaf │       │      │
│    │ │ A-D  │     │ E-O  │ │ P-Q  │ │ R    │ │ S-W  │ │ X-Z  │       │      │
│    │ └──────┘     └──────┘ └──────┘ └──────┘ └──────┘ └──────┘       │      │
│    │    ▲              ▲                         ▲                   │      │
│    │    │              │                         │                   │      │
│    │    └──────────────┴─────────────────────────┘                   │      │
│    │           Leaf pages linked for sequential scan                 │      │
│    └─────────────────────────────────────────────────────────────────┘      │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘

                    THE CURSOR
                    ══════════
      ┌─────────────────────────────────────────────┐
      │  Cursor State:                              │
      │  ┌─────────────────────────────────────┐    │
      │  │ • Current Page Pointer  ──────────────►  │ ──► Points to leaf "E-O"
      │  │ • Position in Page      ──────────────►  │ ──► Index 3 (key "H")
      │  │ • Transaction Reference ──────────────►  │ ──► Read-only TX #42
      │  │ • Internal Buffer       ──────────────►  │ ──► For compression
      │  └─────────────────────────────────────┘    │
      └─────────────────────────────────────────────┘
```

## Cursor Operations Visualized

### SEEK Operation: `cursor.seek("R")`

**Cost: O(log n) - Must traverse tree from root**

```
Step 1: Start at Root          Step 2: Follow "P-R" branch    Step 3: Land on leaf
        ┌─────────┐                    ┌─────────┐                   ┌──────┐
        │ [P,S,Z] │ ◄─ "R" >= "P"      │ [P-R]   │ ◄─ follow         │  R   │ ◄─ FOUND!
        └────┬────┘    but < "S"       └────┬────┘    child ptr      └──────┘
             │                              │                             ▲
             ▼                              ▼                             │
        Go to "P-R"                   Go to "R" leaf              Cursor now HERE
        branch

   Disk I/O: May page-fault on each level if pages not in cache
   Best case (hot cache): Just pointer chasing in memory
   Worst case (cold): 3-4 disk reads (one per tree level)
```

### NEXT Operation: `cursor.next()`

**Cost: O(1) amortized - Just moves within/between leaf pages**

```
CASE 1: Next key in same leaf page (FAST - no tree traversal)
┌────────────────────────────────────┐
│  Leaf Page [E, F, G, H, I, J, K]   │
│                    ▲   ▲           │
│                    │   │           │
│              before│   │after      │
│              (at H)    (at I)      │
└────────────────────────────────────┘
Just increment position index. No I/O.


CASE 2: Next key requires moving to next leaf (still fast)
┌──────────────────┐     ┌──────────────────┐
│  Leaf [... O]    │ ──► │  Leaf [P, Q...] │
│            ▲     │     │  ▲               │
│            │     │     │  │               │
│         before   │     │  after           │
└──────────────────┘     └──────────────────┘
Follow sibling pointer. Usually 1 page access.
Pages are often contiguous/prefetched.
```

### WALK Operation: `cursor.walk(Some(start_key))?`

**Creates an iterator that yields all key-value pairs sequentially**

```
cursor.walk(Some("C"))?
    │
    ▼
┌─────────────────────────────────────────────────────────────────┐
│   1. Initial SEEK to "C"                                        │
│      └── O(log n) tree traversal                                │
│                                                                 │
│   2. Iterator created, cursor positioned                        │
│      └── Cursor now points to "C" in leaf                       │
│                                                                 │
│   3. Each .next() on iterator:                                  │
│      └── Just calls cursor.next() internally                    │
│      └── O(1) - walks leaf chain                                │
│                                                                 │
│   Result: ONE seek + N sequential reads                         │
│           Much better than N seeks!                             │
└─────────────────────────────────────────────────────────────────┘

   Timeline:
   ─────────────────────────────────────────────────────────────────►

   │ seek("C") │ next │ next │ next │ next │ next │ ... │
   │  (slow)   │(fast)│(fast)│(fast)│(fast)│(fast)│     │

   └─── O(log n) ───┘ └────────── O(1) each ─────────────┘
```

## Good vs Bad Cursor Patterns

### BAD: Seek Per Item (Random I/O)

```
for address in addresses {           // 1000 addresses
    cursor.seek_exact(address)?;     // 1000 tree traversals!
}

I/O Pattern:
┌─────┐  ┌─────┐  ┌─────┐  ┌─────┐  ┌─────┐
│Seek │  │Seek │  │Seek │  │Seek │  │Seek │  ... x 1000
│ 0x1 │  │ 0x9 │  │ 0x3 │  │ 0xF │  │ 0x5 │
└─────┘  └─────┘  └─────┘  └─────┘  └─────┘
   ▼        ▼        ▼        ▼        ▼
  Page     Page     Page     Page     Page    ◄── Random pages!
   A        Z        K        M        C           Cache thrashing

Time: O(n × log m) where n=items, m=table size
```

### GOOD: Sort First, Then Sequential Access

```
addresses.sort();                    // Sort first
for address in addresses {
    cursor.seek_exact(address)?;     // Seeks move FORWARD
}

I/O Pattern:
┌─────┐  ┌─────┐  ┌─────┐  ┌─────┐  ┌─────┐
│Seek │  │Seek │  │Seek │  │Seek │  │Seek │  ... (forward only)
│ 0x1 │  │ 0x3 │  │ 0x5 │  │ 0x9 │  │ 0xF │
└─────┘  └─────┘  └─────┘  └─────┘  └─────┘
   ▼        ▼        ▼        ▼        ▼
  Page     Page     Page     Page     Page    ◄── Sequential/nearby!
   A        C        E        K        M           Cache friendly

Or even better - use walk_range() if contiguous!
```

### BEST: Walk Range (Sequential Scan)

```
cursor.walk_range(start..end)?       // ONE seek
    .for_each(|entry| ...);          // Then iterate

I/O Pattern:
┌──────┐  ┌──────┐  ┌──────┐  ┌──────┐  ┌──────┐
│ Seek │  │ next │  │ next │  │ next │  │ next │  ...
│start │  │      │  │      │  │      │  │      │
└──────┘  └──────┘  └──────┘  └──────┘  └──────┘
   ▼         │         │         │         │
  Tree    Follow    Follow    Follow    Follow   ◄── Leaf chain walk
  walk    leaf ptr  leaf ptr  leaf ptr  leaf ptr     (prefetchable!)

Time: O(log m + n) - one seek then linear scan
```

## DUPSORT Tables (Multiple Values Per Key)

```
Regular Table:                    DUPSORT Table:
┌─────────────────────┐          ┌─────────────────────────────────────┐
│ Key    │ Value      │          │ Key     │ SubKey  │ Value           │
├────────┼────────────┤          ├─────────┼─────────┼─────────────────┤
│ Addr1  │ Account1   │          │ Block1  │ Addr1   │ ChangeEntry1    │
│ Addr2  │ Account2   │          │ Block1  │ Addr2   │ ChangeEntry2    │
│ Addr3  │ Account3   │          │ Block1  │ Addr3   │ ChangeEntry3    │
└─────────────────────┘          │ Block2  │ Addr1   │ ChangeEntry4    │
                                 └─────────┴─────────┴─────────────────┘

DUPSORT Cursor Operations:
┌────────────────────────────────────────────────────────────────────┐
│  cursor.seek_exact(Block1)?        // Position at Block1          │
│       │                                                            │
│       ▼                                                            │
│  cursor.next_dup()?                // Next entry, SAME key         │
│       │                            // Block1,Addr1 → Block1,Addr2  │
│       ▼                                                            │
│  cursor.next_dup()?                // Block1,Addr2 → Block1,Addr3  │
│       │                                                            │
│       ▼                                                            │
│  cursor.next_no_dup()?             // Skip to NEXT key             │
│                                    // Block1,Addr3 → Block2,Addr1  │
└────────────────────────────────────────────────────────────────────┘
```

## Write Operations

### APPEND vs UPSERT

```
UPSERT (cursor.upsert): "Find position, then write"
──────────────────────────────────────────────────
1. Seek to find correct position    O(log n)
2. Check if key exists
3. Update or insert

   Use when: Order unknown, may update existing


APPEND (cursor.append): "Write at end, no seeking"
──────────────────────────────────────────────────
1. Go to last position              O(1)
2. Verify key > last key
3. Append

   Use when: Data is pre-sorted, always appending


CRITICAL: Pre-sort your data before writing!

┌────────────────────────────────────────────────────────────────┐
│  // GOOD: Sort → then append                                   │
│  data.sort_by_key(|x| x.key);      // Parallel sort: O(n log n)│
│  for item in data {                                            │
│      cursor.append(item.key, item.value)?;  // O(1) each       │
│  }                                                             │
│  // Total: O(n log n) + O(n) = O(n log n)                      │
│                                                                │
│  // BAD: Upsert unsorted                                       │
│  for item in unsorted_data {                                   │
│      cursor.upsert(item.key, item.value)?;  // O(log m) each   │
│  }                                                             │
│  // Total: O(n log m) where m = table size                     │
└────────────────────────────────────────────────────────────────┘
```

## MMAP: Why "Kernel Calls" is Nuanced

```
Traditional DB:                      MMAP (what MDBX uses):
───────────────                      ─────────────────────

   Application                          Application
       │                                    │
       │ read(fd, buf, n)                   │ ptr = mmap(file)
       ▼                                    │ data = *ptr
   ┌───────┐                                ▼
   │Syscall│ ◄── Always!             ┌─────────────┐
   └───┬───┘                         │ Page Fault? │
       │                             └──────┬──────┘
       ▼                                    │
   ┌───────┐                         ┌──────┴──────┐
   │ Kernel│                         ▼             ▼
   └───┬───┘                     Page in        Page NOT
       │                         Cache          in Cache
       ▼                            │              │
   ┌───────┐                        ▼              ▼
   │ Disk  │                     No I/O!      ┌────────┐
   └───────┘                     (fast)       │Syscall │
                                              └────┬───┘
                                                   ▼
                                              ┌────────┐
                                              │ Disk   │
                                              └────────┘

Key insight: Sequential access → pages prefetched → fewer faults
             Random access → cache misses → more page faults

Your cursor pattern determines the I/O pattern!
```

## Summary

```
┌─────────────────────────────────────────────────────────────────────┐
│                     CURSOR PERFORMANCE RULES                        │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  1. MINIMIZE SEEKS                                                  │
│     • Use walk_range() instead of repeated seek_exact()             │
│     • Sort data before seeking                                      │
│     • One cursor, many iterations > many cursors                    │
│                                                                     │
│  2. PREFER SEQUENTIAL ACCESS                                        │
│     • next() is O(1), seek() is O(log n)                            │
│     • Sequential = cache friendly = fast                            │
│     • Random = cache thrashing = slow                               │
│                                                                     │
│  3. PRE-SORT FOR WRITES                                             │
│     • Sort before writing (use rayon for parallel sort)             │
│     • Use append() for sorted data                                  │
│     • Batch writes, commit once                                     │
│                                                                     │
│  4. REUSE CURSORS                                                   │
│     • Create cursor outside loop                                    │
│     • Cursor creation has overhead                                  │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```
