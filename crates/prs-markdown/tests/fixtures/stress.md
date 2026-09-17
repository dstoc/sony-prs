# Reader stress corpus

This document is intentionally representative rather than pretty. It combines
long prose, many headings, nested lists, highlighted code, a wide table,
several images, and links into another Markdown document.

[Open the linked chapter](stress-linked.md#continuation) and
[jump to the final section](#final-section).

## Prose and navigation

The reader should retain source semantics while keeping only the resources
needed for the current device-sized page. This paragraph repeats enough words
to exercise wrapping, page boundaries, link regions, and the text measurement
path on a small e-ink viewport. The same layout must remain stable when a page
display list is evicted and rebuilt from its logical page directory.

## Nested lists

- First top-level item with a deliberately long continuation that wraps.
  - Nested item with another continuation and a [chapter link](stress-linked.md).
    - Deep item with enough words to cross a line boundary.
- Second top-level item
  - Nested item

1. Ordered item one
2. Ordered item two
   1. Nested ordered item
   2. Another nested ordered item

## Highlighted code

```rust
fn bounded_page_turns(source: &str, page: usize) -> Result<(), &'static str> {
    let cache = page.saturating_add(1);
    if source.is_empty() || cache == 0 {
        return Err("empty source");
    }
    println!("rebuild page {page} from the compact index");
    Ok(())
}
```

```json
{"document":"stress.md","pages":128,"cache":{"pages":3,"documents":2}}
```

## Wide table

| Identifier | Status | Owner | Description | Latency | Memory |
| --- | :--- | ---: | --- | ---: | ---: |
| reader | ready | host | page directory and bounded display-list cache | 12 ms | 1 MiB |
| parser | ready | shared | owned Markdown IR with no arena escape | 18 ms | 2 MiB |
| layout | ready | shared | proportional metrics and table continuation | 31 ms | 3 MiB |
| renderer | ready | T1 | generic RGB565 draw-target boundary | 7 ms | 256 KiB |

## Several images

![Observatory landscape](assets/observatory.png)

![Repeated observatory image](assets/observatory.png)

![Inline image](assets/observatory.png) continues with text so the inline
fallback and loaded-raster path are both represented in the same corpus.

## More headings

### Heading three

Short section text.

### Heading four

More section text and a [local anchor](#final-section).

### Heading five

More section text.

## Final section

The final section lets link activation and history restoration use a non-zero
page cursor in the stress run.
