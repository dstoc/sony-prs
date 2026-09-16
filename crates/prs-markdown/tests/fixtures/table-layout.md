# Agent result matrix

The following compact table mixes ordinary prose, inline code, links, and
alignment hints in the form commonly returned by an agent.

| Check | Result | Notes |
| :--- | :---: | ---: |
| Parser | **passed** | `ComrakParser` preserves links and inline styles. |
| Layout | *passed* | Long prose wraps inside the cell instead of escaping the viewport. |
| Resources | passed | See [the resource policy](docs/resources.md#roots). |
| Identifiers | passed | `sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef` |

## Wide diagnostic record

| Key | State | Owner | Area | Build | Device | Link | Detail |
| --- | :---: | --- | --- | --- | --- | --- | --- |
| MR-11 | ready | reader | layout | host-linux | PRS-T1 | [review](https://example.invalid/sony-prs/14) | This deliberately long diagnostic explanation spans enough lines to force the table across several pages while keeping the identifying key visible. |
| MR-12 | next | renderer | display | host-linux | PRS-350 | [notes](https://example.invalid/sony-prs/12) | A second row makes row-boundary pagination and repeated headers observable in the host regression harness. |
| MR-13 | ready | images | resources | host-linux | PRS-T1 | [image](https://example.invalid/sony-prs/13) | `assets/very-long-image-reference-name.webp` remains readable through deterministic wrapping. |
