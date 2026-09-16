# Markdown reader regression corpus

This is ordinary prose with **strong emphasis**, *italic emphasis*, and
`inline code`. The paragraph is intentionally long enough to wrap at a narrow
viewport so page boundaries exercise visible fragment preservation.

## Navigation

This [local chapter](linked-chapter.md) opens another file, while this
[anchored chapter](linked-chapter.md#installation) targets a heading in it.
This [local anchor](#navigation) stays in the current document, and this
[external reference](https://example.com/reader) remains external.

## Nested lists and tasks

1. First ordered item
   - nested unordered item
     - deeply nested item
   - another nested item
2. Second ordered item

- [x] Completed task
  - [ ] Nested open task
- [ ] Open task

## Quoted response

> A block quote is still readable in the host output.
>
> It may contain **styled text** and a [link](linked-chapter.md#details).

## Table placeholder

| Field | Value | Notes |
| :--- | :----: | ---: |
| parser | ready | owned document |
| layout | ready | line boundaries |
| renderer | host | PGM output |

## Fenced code

```rust
fn render_page(page: &PageLayout) {
    println!("page {}", page.number);
}
```

## Image reference

![A diagram placeholder](images/reader-flow.png "asset example")

---

The end of the corpus provides one more paragraph after the thematic rule.
