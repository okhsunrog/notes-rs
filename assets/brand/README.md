# Tangleaf icons

The external icon uses the Iris palette from the brand archive: `#29205c`,
`#5738a5`, `#7856d9`, `#ac8bff`. The original leaf geometry and gradient
intensity are preserved. In-app marks still follow the selected UI palette.

Run `vp run icons:generate` from the repository root to regenerate the favicon,
desktop/iOS icons and Android resources from `tangleaf.svg`.

Android uses a pale Iris background and a generated foreground with 50% artwork
scale in its 108dp layer. This keeps the pointed leaf and T visible under circular
and rounded-square masks. The manifest's 150% scale applies only to legacy icons,
compensating for the adaptive layer's outer margins. Do not run the icon generator
directly on the unpadded master SVG: it fills the adaptive layer and gets clipped.

Android 13+ uses the existing vector monochrome with transparent T cutouts, aligned
to the same foreground scale (`108 / 1120`, translated by 27dp). Its colors are
assigned by the launcher. If changing padding, update that vector transform too.
