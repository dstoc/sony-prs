# Release checklist

The **reader** supports *nested* Markdown, ~~legacy~~ notes, `inline code`,
a [design doc](docs/design.md "Design") and an image ![diagram](img/flow.png).
Soft breaks stay here\
while hard breaks end this line.

## Release checklist

- [x] Parse the source
  - [ ] Preserve nested tasks
- [ ] Review the output

> Ship the small reader first.
>
> Then measure it.

```rust,ignore
fn main() { println!("hello"); }
```

| Name | Status | Owner |
| :--- | :----: | ---: |
| Parser | ready | Ada |
| Layout | next | Grace |

---
