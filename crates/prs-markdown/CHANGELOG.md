# Changelog

## [0.8.0](https://github.com/dstoc/sony-prs/compare/prs-markdown-v0.7.0...prs-markdown-v0.8.0) (2026-09-23)


### Features

* **prs-markdown:** make reader core wasm-safe ([90493f4](https://github.com/dstoc/sony-prs/commit/90493f467bcd0ff5c01a94bda912da5640210368))
* **reader-web:** add directory-backed browser library ([ee6317d](https://github.com/dstoc/sony-prs/commit/ee6317df4e5acd21b5918fd7a2486216bfd36926))
* **reader:** add bounded font size controls ([99f61a7](https://github.com/dstoc/sony-prs/commit/99f61a7f8f8abd5aa322c0eaadc71f54551bd830))
* **reader:** add per-document progress indicator ([778f9e0](https://github.com/dstoc/sony-prs/commit/778f9e0e141b6c97bb7670570b23c29702081a5b))
* **reader:** add portrait and landscape orientation ([0f9ea8a](https://github.com/dstoc/sony-prs/commit/0f9ea8af88aad97a954ee6acb7f9730bd3e60423))
* **reader:** preserve logical position across reflow ([b20ddb8](https://github.com/dstoc/sony-prs/commit/b20ddb8a9d671d54ee8f4a9c7e9b96c8e743f03c))
* **reader:** share configurable presentation layout ([a9a68c7](https://github.com/dstoc/sony-prs/commit/a9a68c71443f924dc64be49c9b65a758cd967ee2))
* **reader:** share external-link overlay session ([ceaf641](https://github.com/dstoc/sony-prs/commit/ceaf6413ee7a9127991c7eefa20147ae176d2e19))


### Bug Fixes

* **reader:** align reflow anchors with rendered content ([9af5d0f](https://github.com/dstoc/sony-prs/commit/9af5d0fe8ec89b21c8fd0a330daeaaac1ce3c938))
* **reader:** align table reflow anchors ([e118cd9](https://github.com/dstoc/sony-prs/commit/e118cd96d4574f117dcac4b0d557c2e7b490a3bd))
* **reader:** map grouped table anchors ([aa6a2f5](https://github.com/dstoc/sony-prs/commit/aa6a2f550590fd9071da5c7b2c5865abfdd79983))

## [0.7.0](https://github.com/dstoc/sony-prs/compare/prs-markdown-v0.6.1...prs-markdown-v0.7.0) (2026-09-18)


### Features

* **t1:** build reader for ARMv7 Cortex-A8 ([66bf99a](https://github.com/dstoc/sony-prs/commit/66bf99a98664a2f8f998f0792c1e22fac04be0a3))

## [0.6.1](https://github.com/dstoc/sony-prs/compare/prs-markdown-v0.6.0...prs-markdown-v0.6.1) (2026-09-17)


### Bug Fixes

* **prs-markdown:** normalize table cell line spacing ([400bdcc](https://github.com/dstoc/sony-prs/commit/400bdccb1871d1d58eeae92b3ec9c1da4e7cda14))

## [0.6.0](https://github.com/dstoc/sony-prs/compare/prs-markdown-v0.5.0...prs-markdown-v0.6.0) (2026-09-17)


### Features

* **prs-markdown:** honor tight and loose list spacing ([c86aa56](https://github.com/dstoc/sony-prs/commit/c86aa56e75738edc80611315c096fdd9a2209037))
* **prs-markdown:** render task list checkboxes ([bc3d5e5](https://github.com/dstoc/sony-prs/commit/bc3d5e542a19f1401a96535cc021b45c00f89ed1))
* **prs-markdown:** underline linked text ([adf3a52](https://github.com/dstoc/sony-prs/commit/adf3a52e26f2d2b53ceec1e3378df79b53d15e7b))


### Bug Fixes

* **markdown:** preserve ordered list start numbers ([49d0923](https://github.com/dstoc/sony-prs/commit/49d0923333d185cc8ee97039de4a4c1f90a94a48))
* **prs-markdown:** composite text against surface backgrounds ([d4079a3](https://github.com/dstoc/sony-prs/commit/d4079a344dfda9509eee002e10fcd419ad4c6d3c))
* **prs-markdown:** draw table grid boundaries once ([d51d381](https://github.com/dstoc/sony-prs/commit/d51d3815f2b79b3d38a11c27d8661c3e4a1f9c0c))
* **prs-markdown:** keep markers with block list items ([8003c5d](https://github.com/dstoc/sony-prs/commit/8003c5d497685f92f88b43f1504a37afc89725f6))
* **prs-markdown:** keep task markers in stable columns ([20c298f](https://github.com/dstoc/sony-prs/commit/20c298f3b2d7c8954439af9ef4c907b0c1c64358))
* **prs-markdown:** restore grouped page-start rule ([913143f](https://github.com/dstoc/sony-prs/commit/913143f3c85c684fca7dcbed4ee738fd604baed2))
* **prs-markdown:** stabilize list marker columns ([6474f89](https://github.com/dstoc/sony-prs/commit/6474f894d1334b751cd8dabc2da1fb205b31562f))
* **prs-markdown:** stabilize list marker columns ([5484d5a](https://github.com/dstoc/sony-prs/commit/5484d5a9cd8078815701cfb424870bfe1f66c812))
* **prs-markdown:** use one fill for wrapped code lines ([0682530](https://github.com/dstoc/sony-prs/commit/068253022414fb93ad833d253629b6143ecc5d25))

## [0.5.0](https://github.com/dstoc/sony-prs/compare/prs-markdown-v0.4.0...prs-markdown-v0.5.0) (2026-09-17)


### Features

* **prs-markdown:** use real monospace style faces ([522aa3a](https://github.com/dstoc/sony-prs/commit/522aa3a881aa4ece7fcf3a56997fdcec764c1eb4))
* **prs-markdown:** use upstream Syntect grammars ([3d65a3f](https://github.com/dstoc/sony-prs/commit/3d65a3f8d4acc2ceb364808f64b15c89f2244bfd))


### Bug Fixes

* **prs-markdown:** add stable code block surfaces ([cd84656](https://github.com/dstoc/sony-prs/commit/cd846564b72501b36d1766859b61dd5b13e08d9b))
* **prs-markdown:** allocate wide table columns by content ([bb37cf3](https://github.com/dstoc/sony-prs/commit/bb37cf362bc15e9ad88440f4e903552feb175eb1))
* **prs-markdown:** improve e-ink syntax contrast ([4efa5e9](https://github.com/dstoc/sony-prs/commit/4efa5e977e0086a1b712c3e9a797b6b8ee3a1bf3))
* **prs-markdown:** improve table typography and padding ([073663d](https://github.com/dstoc/sony-prs/commit/073663d16b8eda395b5e92fd6c1795bebe4fe434))
* **prs-markdown:** keep wrapped table rows continuous ([ec5266e](https://github.com/dstoc/sony-prs/commit/ec5266e8e8183937db538964860e5f6ecac5c319))
* **prs-markdown:** preserve nested code surfaces ([07f6a54](https://github.com/dstoc/sony-prs/commit/07f6a5418c14cd4c2b8546b49415787b3c61f5b2))
* **prs-markdown:** preserve wrapped header border width ([fae4cf3](https://github.com/dstoc/sony-prs/commit/fae4cf30d2ae0a5498405373ce13894907e092ca))
* **prs-markdown:** render italic syntax in code ([2132d32](https://github.com/dstoc/sony-prs/commit/2132d3208ec77d600f04d766c573b5ddb6adcf56))
* **prs-markdown:** try normal table typography first ([161a013](https://github.com/dstoc/sony-prs/commit/161a013032f150c571141bbf29991705442b1c66))

## [0.4.0](https://github.com/dstoc/sony-prs/compare/prs-markdown-v0.3.0...prs-markdown-v0.4.0) (2026-09-17)


### Features

* bound reader memory and page caches ([b080b19](https://github.com/dstoc/sony-prs/commit/b080b19e03a0f2eaac6fb3a2236513e0e7e8a16e))

## [0.3.0](https://github.com/dstoc/sony-prs/compare/prs-markdown-v0.2.0...prs-markdown-v0.3.0) (2026-09-17)


### Features

* add bounded syntax highlighting for code blocks ([05bd18b](https://github.com/dstoc/sony-prs/commit/05bd18bad91882c8c7db1a978e5b0e40439b82cf))
* add deterministic markdown pagination ([9c27bab](https://github.com/dstoc/sony-prs/commit/9c27bab9b2c16979df1c5703770004da151c9604))
* add high-level markdown reader controller ([d89835f](https://github.com/dstoc/sony-prs/commit/d89835ff9a23f73f02b3b43e00379d9d05dc1677))
* add host markdown rendering harness ([4ff43fa](https://github.com/dstoc/sony-prs/commit/4ff43fa009dd1607566a363aff7c683c1bbbd007))
* add markdown resource and path resolution ([a8cfc1a](https://github.com/dstoc/sony-prs/commit/a8cfc1ac64b45854f19cf65cccf49a7ada077981))
* complete modern markdown fallbacks ([e59ae62](https://github.com/dstoc/sony-prs/commit/e59ae6205cdf57a2b7158479d4bb15a39f2717ff))
* define markdown page display-list geometry ([31ba828](https://github.com/dstoc/sony-prs/commit/31ba8288294fa20a321130494abe4839a19fe868))
* establish reusable markdown reader architecture ([71a8f04](https://github.com/dstoc/sony-prs/commit/71a8f043f99951b8571d98534aa02774c1f5315a))
* implement markdown block layout ([0519ad2](https://github.com/dstoc/sony-prs/commit/0519ad25888e200a5ec6b17bf38958e70abf3c90))
* parse GFM into owned markdown documents ([0f842e7](https://github.com/dstoc/sony-prs/commit/0f842e7623cb9e90558587c5f736633eb145a6d0))
* **prs-markdown:** add raster image support ([bc5bf22](https://github.com/dstoc/sony-prs/commit/bc5bf22a661b3979681e100707a4a91dd64c3c67))
* render markdown page layouts ([238805c](https://github.com/dstoc/sony-prs/commit/238805c840ec9daae034dfc3b50b5ffaf01c42fd))


### Bug Fixes

* **prs-markdown:** use Oniguruma for runtime highlighting ([cc460c3](https://github.com/dstoc/sony-prs/commit/cc460c315bd7f4f728a037037fab557ad113dc47))

## [0.2.0](https://github.com/dstoc/sony-prs/compare/prs-markdown-v0.1.0...prs-markdown-v0.2.0) (2026-09-17)


### Features

* add bounded syntax highlighting for code blocks ([05bd18b](https://github.com/dstoc/sony-prs/commit/05bd18bad91882c8c7db1a978e5b0e40439b82cf))
* add deterministic markdown pagination ([9c27bab](https://github.com/dstoc/sony-prs/commit/9c27bab9b2c16979df1c5703770004da151c9604))
* add high-level markdown reader controller ([d89835f](https://github.com/dstoc/sony-prs/commit/d89835ff9a23f73f02b3b43e00379d9d05dc1677))
* add host markdown rendering harness ([4ff43fa](https://github.com/dstoc/sony-prs/commit/4ff43fa009dd1607566a363aff7c683c1bbbd007))
* add markdown resource and path resolution ([a8cfc1a](https://github.com/dstoc/sony-prs/commit/a8cfc1ac64b45854f19cf65cccf49a7ada077981))
* complete modern markdown fallbacks ([e59ae62](https://github.com/dstoc/sony-prs/commit/e59ae6205cdf57a2b7158479d4bb15a39f2717ff))
* define markdown page display-list geometry ([31ba828](https://github.com/dstoc/sony-prs/commit/31ba8288294fa20a321130494abe4839a19fe868))
* establish reusable markdown reader architecture ([71a8f04](https://github.com/dstoc/sony-prs/commit/71a8f043f99951b8571d98534aa02774c1f5315a))
* implement markdown block layout ([0519ad2](https://github.com/dstoc/sony-prs/commit/0519ad25888e200a5ec6b17bf38958e70abf3c90))
* parse GFM into owned markdown documents ([0f842e7](https://github.com/dstoc/sony-prs/commit/0f842e7623cb9e90558587c5f736633eb145a6d0))
* **prs-markdown:** add raster image support ([bc5bf22](https://github.com/dstoc/sony-prs/commit/bc5bf22a661b3979681e100707a4a91dd64c3c67))
* render markdown page layouts ([238805c](https://github.com/dstoc/sony-prs/commit/238805c840ec9daae034dfc3b50b5ffaf01c42fd))


### Bug Fixes

* **prs-markdown:** use Oniguruma for runtime highlighting ([cc460c3](https://github.com/dstoc/sony-prs/commit/cc460c315bd7f4f728a037037fab557ad113dc47))
