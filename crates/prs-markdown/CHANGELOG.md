# Changelog

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
