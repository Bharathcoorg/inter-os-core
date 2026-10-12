# What a higher scanout would actually cost

Measured from the repo without booting anything. This is the answer to "make it
full HD", and it is not the answer the request expected.

## The numbers

| Scanout | Pixels | MB per ARGB8888 surface | Compositor front+back | % of the 64 MB heap |
|---|---|---|---|---|
| 720x1440 (shipped) | 1,036,800 | 3.96 MB | 7.91 MB | **12.4%** |
| 900x1800 | 1,620,000 | 6.18 MB | 12.36 MB | 19.3% |
| 1080x2400 (FHD-class) | 2,592,000 | 9.89 MB | 19.78 MB | **30.9%** |

`MAX_SURFACE_PIXELS` already grants an explicit exception for `width <= 1080 &&
height <= 2400`, so the kernel does not block FHD. The question is what it costs.

The compositor's two buffers are not the whole cost. In the realistic case -- the
launcher surface, one foreground app surface, and the 720x96 system line --

| Scanout | Surfaces | Buffers | Total | % of heap |
|---|---|---|---|---|
| 720x1440 | 8.17 MB | 7.91 MB | **16.08 MB** | 25.1% |
| 1080x2400 | 20.04 MB | 19.78 MB | **39.81 MB** | 62.2% |

That is 62% of the heap on scanout alone, before any app data, any file buffer,
any IPC, or any of the kernel's own structures. `MAX_CLIENT_SURFACES` is 32, and
32 phone-sized surfaces at FHD would be 316 MB -- which is why FHD would also
force the surface cap down, not just the buffers up.

## Why this is the wrong trade for this OS

The stated competitive advantage is small size and low RAM. Spending 62% of the
heap on scanout spends that advantage to buy resolution the guest does not
currently need: the text-clarity problem that motivated the request was not a
resolution problem, and it is now fixed. A 2x crop of the home surface's "Today"
carried 15 distinct tones before the outline rasteriser and 52 after, because
the edge is drawn where the outline puts it rather than stretched from a 16x24
bitmap cell.

## What raising the scanout would not fix

Device Studio presents the guest's 720x1440 canvas at roughly 0.457x. Raising
the guest resolution makes that ratio *worse*, not better: a 1080-wide frame in
the same chassis is presented at 0.305x. Presentation scale is a property of the
chassis, and the guest-side levers all trade away the one advantage this OS has.

## The round-trip constraint

`typeface::raster` takes an integer pixel origin, so the authoring-to-scanout
ratio must divide evenly for every glyph advance to stay an integer:

- 1080/720 = 1.5 -- **not** an integer; rounds every glyph advance
- 1440/720 = 2 -- integer, but 2880 rows is past both the kernel's exception and the heap
- 1 (shipped) -- exact

`design::viewport::logical_to_surface` and `surface_to_logical` now exist so the
ratio is a single decision, and `viewport::scaled()` lets a caller detect the
state and refuse the mismatch rather than draw a correctly laid out surface with
undersized text.

## Recommendation

Do not raise the scanout. The clarity that motivated the request is delivered,
and the cost of the resolution it was asked for is a majority of the heap. If a
higher scanout is ever needed, the order is: narrow the compositor to dirty rows
first, because that is what makes a bigger surface affordable, and it is
independently worth doing -- `SurfaceWrite` now records its dirty rows, so the
compositor's full-buffer row diff is no longer load-bearing.
