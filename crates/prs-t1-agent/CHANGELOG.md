# Changelog

## [0.12.0](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.11.1...prs-t1-agent-v0.12.0) (2026-09-23)


### Features

* **prs-t1-agent:** add network-loss fault injection ([cf5c4d5](https://github.com/dstoc/sony-prs/commit/cf5c4d566bb04a8e13ba2360740f376551e293a8))
* **prs-t1-agent:** add reader authorization and bundle sync ([38b52a1](https://github.com/dstoc/sony-prs/commit/38b52a1a75fc7eac66fd0568cc46fa01cbbef6b7))
* **prs-t1-agent:** add synchronization trigger policy ([3a7e6d8](https://github.com/dstoc/sony-prs/commit/3a7e6d8c612ec2f5a7db1c0256b8f07ea18fd102))
* **prs-t1-agent:** integrate synced reader settings ([0579f62](https://github.com/dstoc/sony-prs/commit/0579f62aad1e5f0e5e3287b653aa3065c0b8fa10))
* **prs-t1-agent:** redesign details actions and press feedback ([13f82f1](https://github.com/dstoc/sony-prs/commit/13f82f1a0f7ebec7a239da11294acea214cdaa7c))
* **prs-t1-agent:** refine reader feedback and sync status ([f30e049](https://github.com/dstoc/sony-prs/commit/f30e049a4d7f886773aaf048f5a15a5d40fc6353))
* **prs-t1-agent:** show external links as QR overlays ([eb6d6e5](https://github.com/dstoc/sony-prs/commit/eb6d6e5c6fee0afd915920d388d7070e5d0d50b9))
* **prs-t1-agent:** sync once after sleep wake ([4cd9d37](https://github.com/dstoc/sony-prs/commit/4cd9d3751459f1800964f6472e2d86ebf002776a))
* **prs-t1:** add hardware settings navigation ([f93e1e9](https://github.com/dstoc/sony-prs/commit/f93e1e94291a600e80327199b091cdaf1c1b9844))
* **reader-web:** add one-shot PRSync browser sync ([6f63baa](https://github.com/dstoc/sony-prs/commit/6f63baa2d31a3f700f6740a7d445875ac5dfafc5))
* **reader:** add bounded font size controls ([99f61a7](https://github.com/dstoc/sony-prs/commit/99f61a7f8f8abd5aa322c0eaadc71f54551bd830))
* **reader:** add fullscreen reading mode ([3e154ae](https://github.com/dstoc/sony-prs/commit/3e154aeb3724b318db5e4c05c3f0909659ae9027))
* **reader:** add per-document progress indicator ([778f9e0](https://github.com/dstoc/sony-prs/commit/778f9e0e141b6c97bb7670570b23c29702081a5b))
* **reader:** add portrait and landscape orientation ([0f9ea8a](https://github.com/dstoc/sony-prs/commit/0f9ea8af88aad97a954ee6acb7f9730bd3e60423))
* **reader:** share configurable presentation layout ([a9a68c7](https://github.com/dstoc/sony-prs/commit/a9a68c71443f924dc64be49c9b65a758cd967ee2))
* **reader:** share external-link overlay session ([ceaf641](https://github.com/dstoc/sony-prs/commit/ceaf6413ee7a9127991c7eefa20147ae176d2e19))
* **t1:** persist reader preferences ([a5991e6](https://github.com/dstoc/sony-prs/commit/a5991e6763b16c0935d87c93baedffbe4bc48c2d))
* **t1:** polish settings navigation and layout ([ae41ce2](https://github.com/dstoc/sony-prs/commit/ae41ce22f9a63670d3ef3cdbada0ddb19006db1d))
* **t1:** redesign settings around reading preferences ([ca7ca59](https://github.com/dstoc/sony-prs/commit/ca7ca59af2786ce564ac7ac485e3f6f3f438988c))


### Bug Fixes

* **prs-t1-agent:** adopt synced bundle from placeholder ([df5e674](https://github.com/dstoc/sony-prs/commit/df5e674133ed7a70cdad07f90442cb9f996ee6d6))
* **prs-t1-agent:** bound immediate sync failure refresh ([c6dcab0](https://github.com/dstoc/sony-prs/commit/c6dcab0efea43b088f40f68678fb504fe876be5c))
* **prs-t1-agent:** bound raw bundle staging ([8950624](https://github.com/dstoc/sony-prs/commit/895062494140b7aebaf7cc2502ad3c3d78e998e5))
* **prs-t1-agent:** clean up sync cancellation and settings bounds ([f5103e5](https://github.com/dstoc/sony-prs/commit/f5103e5fded072514ef1b8e8dd0b81ee0329dbb0))
* **prs-t1-agent:** clear local library for empty inbox ([4c2adbd](https://github.com/dstoc/sony-prs/commit/4c2adbd9d1cbf3fae4d02fdc56ce2d3aa676b9a2))
* **prs-t1-agent:** document network-loss option in usage ([4acb0ef](https://github.com/dstoc/sony-prs/commit/4acb0efee868f59552c01b1e363a287c036aacb8))
* **prs-t1-agent:** establish portrait framebuffer orientation ([97871d6](https://github.com/dstoc/sony-prs/commit/97871d61aabc48c0910aa93386713bbfc1f0c408))
* **prs-t1-agent:** initialize framebuffer before timeout parsing ([028f770](https://github.com/dstoc/sony-prs/commit/028f770ddfaca9be5e4ee35cca8f0f6a6a6155d3))
* **prs-t1-agent:** keep production details within action pane ([4a33515](https://github.com/dstoc/sony-prs/commit/4a3351555499517889caa5568b9d29f0714929aa))
* **prs-t1-agent:** make reader sync cooperative ([f9537ae](https://github.com/dstoc/sony-prs/commit/f9537ae1b94e9f5f1ce18d445048adc06d61eb1d))
* **prs-t1-agent:** make refreshes damage-driven ([2d6de91](https://github.com/dstoc/sony-prs/commit/2d6de91acd438f6538515ecba54e240cfde151e4))
* **prs-t1-agent:** open framebuffer before Wi-Fi sync ([43d3277](https://github.com/dstoc/sony-prs/commit/43d3277819d8e74710276d947c0b57502580e674))
* **prs-t1-agent:** preserve wrapped external URL text ([140df35](https://github.com/dstoc/sony-prs/commit/140df35593fdd20216c939e51e8eca003012848c))
* **prs-t1-agent:** refine physical button navigation ([6af0457](https://github.com/dstoc/sony-prs/commit/6af04575621c7ac8be1db0380aa261bab55153a6))
* **prs-t1-agent:** repaint feedback after home input ([a666e18](https://github.com/dstoc/sony-prs/commit/a666e18443544da5a39b9bdd1a6e6903248d1cdb))
* **prs-t1-agent:** resolve legacy WPA control sockets ([e26caa7](https://github.com/dstoc/sony-prs/commit/e26caa7fe6fefd3887dfecf0baea32b4597a3dab))
* **prs-t1-agent:** retain merged details status rows ([d2f8608](https://github.com/dstoc/sony-prs/commit/d2f8608b6d3326402b9b193152b8f68707879d37))
* **prs-t1-agent:** reuse TLS provider across sync attempts ([dcd7c15](https://github.com/dstoc/sony-prs/commit/dcd7c15942756b1bc0f3b9f10f8581f4f4b682c4))
* **prs-t1-agent:** use PRS-T1 Wi-Fi socket fallback ([dc1a49e](https://github.com/dstoc/sony-prs/commit/dc1a49e7dfcd57d9ad474dc28488536ffdca2de9))
* **prs-t1-agent:** wait after sync completion before idle retry ([ce83ecc](https://github.com/dstoc/sony-prs/commit/ce83eccdff82ef997f2d2e754ba30c1a69146f74))
* **prsync:** drop per-file manifest hashes ([720899b](https://github.com/dstoc/sony-prs/commit/720899b674eaa35ce0aff2666b54843ab2eac52d))
* **reader:** preserve orientation layout and multitouch mapping ([f3420c7](https://github.com/dstoc/sony-prs/commit/f3420c78d5141692b5607511b45a365b24131ac3))
* **reader:** preserve settings preferences after rebase ([4d183cd](https://github.com/dstoc/sony-prs/commit/4d183cd7ecfee19eb26c59da1777878e4600b256))
* **t1:** align landscape settings row labels ([05b4524](https://github.com/dstoc/sony-prs/commit/05b4524bdc38ad87b57a344252140904f51fd898))
* **t1:** allow font preference without bundle ([fc07559](https://github.com/dstoc/sony-prs/commit/fc07559255fd1f52e5271eac9e9025eab819d041))

## [0.11.1](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.11.0...prs-t1-agent-v0.11.1) (2026-09-21)


### Bug Fixes

* **prs-t1-agent:** make invalid hostname TLS test deterministic ([262c1ba](https://github.com/dstoc/sony-prs/commit/262c1bac597b574488dd82bd11d5ca70c4702f40))

## [0.11.0](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.10.1...prs-t1-agent-v0.11.0) (2026-09-20)


### Features

* **prs-t1-agent:** add Wi-Fi lifecycle probe ([d02dc48](https://github.com/dstoc/sony-prs/commit/d02dc482d92687605f0a1080ad8b7dd0d3b55628))


### Bug Fixes

* **prs-t1-agent:** wait for DHCP shutdown ([206b2ef](https://github.com/dstoc/sony-prs/commit/206b2efc1ffc711f2af6c3f6ba299359bbdca7ab))

## [0.10.1](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.10.0...prs-t1-agent-v0.10.1) (2026-09-20)


### Bug Fixes

* **prs-t1-agent:** initialize secure TLS entropy ([191d06d](https://github.com/dstoc/sony-prs/commit/191d06d8cfd2ca2e451b14c71315d9c547b787ae))

## [0.10.0](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.9.0...prs-t1-agent-v0.10.0) (2026-09-19)


### Features

* **prs-t1-agent:** add network capability probe ([7c81c70](https://github.com/dstoc/sony-prs/commit/7c81c706e57ef26b7d026fe46821bf7c76a0a13d))


### Bug Fixes

* **prs-t1-agent:** avoid probe worker threads ([5fe1c00](https://github.com/dstoc/sony-prs/commit/5fe1c00979fd0ccd37fc4dfcab680e15cf47f2a0))
* **prs-t1-agent:** bound DNS and validate negative probe ([e0d6f9d](https://github.com/dstoc/sony-prs/commit/e0d6f9d28f5d3eaea3d77ff2f31cffabbddb5494))

## [0.9.0](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.8.0...prs-t1-agent-v0.9.0) (2026-09-18)


### Features

* **t1:** open display test from details ([341cbfc](https://github.com/dstoc/sony-prs/commit/341cbfc6b695f4c601b96a01aa9f84d9f14939f6))

## [0.8.0](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.7.0...prs-t1-agent-v0.8.0) (2026-09-18)


### Features

* **t1:** add display calibration test ([4dde8c3](https://github.com/dstoc/sony-prs/commit/4dde8c3352943fe03818df8cf56107e578a22a00))

## [0.7.0](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.6.1...prs-t1-agent-v0.7.0) (2026-09-18)


### Features

* **t1:** build reader for ARMv7 Cortex-A8 ([66bf99a](https://github.com/dstoc/sony-prs/commit/66bf99a98664a2f8f998f0792c1e22fac04be0a3))

## [0.6.1](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.6.0...prs-t1-agent-v0.6.1) (2026-09-17)

## [0.6.0](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.5.0...prs-t1-agent-v0.6.0) (2026-09-17)


### Features

* **prs-markdown:** render task list checkboxes ([bc3d5e5](https://github.com/dstoc/sony-prs/commit/bc3d5e542a19f1401a96535cc021b45c00f89ed1))

## [0.5.0](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.4.3...prs-t1-agent-v0.5.0) (2026-09-17)


### Features

* **prs-markdown:** use real monospace style faces ([522aa3a](https://github.com/dstoc/sony-prs/commit/522aa3a881aa4ece7fcf3a56997fdcec764c1eb4))

## [0.4.3](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.4.2...prs-t1-agent-v0.4.3) (2026-09-17)

## [0.4.2](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.4.1...prs-t1-agent-v0.4.2) (2026-09-17)

## [0.4.1](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.4.0...prs-t1-agent-v0.4.1) (2026-09-17)

## [0.4.0](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.3.0...prs-t1-agent-v0.4.0) (2026-09-17)


### Features

* **t1:** tune document refresh policy ([191d344](https://github.com/dstoc/sony-prs/commit/191d3441dadc5ed50a1d30f1bcdd3cf05391c904))
* **t1:** wire reader controls and navigation ([abcfa31](https://github.com/dstoc/sony-prs/commit/abcfa31d4c3590f275598f042dd3e7a29d1fae09))


### Bug Fixes

* **t1:** provide soft-float math compatibility symbols ([85678d6](https://github.com/dstoc/sony-prs/commit/85678d66c4c03e2d41732dbf6df41cd1aea8d6db))

## [0.3.0](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.2.0...prs-t1-agent-v0.3.0) (2026-09-17)


### Features

* integrate Markdown reader into T1 UI ([fa16905](https://github.com/dstoc/sony-prs/commit/fa169055a09499f5ee451e48ac280990cd6e68c2))


### Bug Fixes

* **release:** update Rust toolchain pin ([73a8c4a](https://github.com/dstoc/sony-prs/commit/73a8c4a1ac343b84b6866d181ae24c2dce587127))
* **t1:** default reader document to sdcard ([7467402](https://github.com/dstoc/sony-prs/commit/74674029eac92853b6a278df0067d91026035a1e))

## [0.2.0](https://github.com/dstoc/sony-prs/compare/prs-t1-agent-v0.1.0...prs-t1-agent-v0.2.0) (2026-09-16)


### Features

* add bounded PRS-T1 evdev logger ([a709d80](https://github.com/dstoc/sony-prs/commit/a709d8022998a300fdc9e9c10f318de8a4fd122e))
* add PRS-T1 read-only display probes ([5b8a454](https://github.com/dstoc/sony-prs/commit/5b8a454f4cc8bb145800a4118cc899675a1a5977))
* add PRS-T1 standalone power and input test ([778e1cb](https://github.com/dstoc/sony-prs/commit/778e1cb5347d60a6878c3be3f29864f3792f9400))
* add read-only T1 status snapshot ([7dafe86](https://github.com/dstoc/sony-prs/commit/7dafe86bfb63c8d370e4aa03b13e35b8c0e6fc48))
* add reversible PRS-T1 framebuffer render test ([74aa79f](https://github.com/dstoc/sony-prs/commit/74aa79f0244786f7aeb9fbe89539afd7a8f7738a))
* add safe T1 native test launcher ([1e3d7b1](https://github.com/dstoc/sony-prs/commit/1e3d7b1be3d7d41d6aadc3a698ab062a22eafb06))
* add T1 waveform refresh probe ([2d1dd4c](https://github.com/dstoc/sony-prs/commit/2d1dd4cf21e394ee69a1e2eb69b7d9a383455ce0))
* automate prs-t1-agent releases ([82b8024](https://github.com/dstoc/sony-prs/commit/82b80242382e1857995fbcf28b73468c2e22a51d))
* bridge T1 power state through vendor API ([fa5a505](https://github.com/dstoc/sony-prs/commit/fa5a50566d90605020a3d23e00b3c7382db4e129))
* establish reusable markdown reader architecture ([71a8f04](https://github.com/dstoc/sony-prs/commit/71a8f043f99951b8571d98534aa02774c1f5315a))
* extend T1 status snapshot ([81ad9f8](https://github.com/dstoc/sony-prs/commit/81ad9f8d011a4cb98dc8ac5f84c922e4ecb8dab1))
* guard native UI ownership ([1c5a877](https://github.com/dstoc/sony-prs/commit/1c5a877f80edd850a7806a7e2d3dac967d212966))
* identify T1 sub-CPU device path ([50c35d6](https://github.com/dstoc/sony-prs/commit/50c35d6b9b1a0af71c457d21786427bd5b20ac4a))
* label native input diagnostics ([86c9751](https://github.com/dstoc/sony-prs/commit/86c97518b99448be749862463b5f37f3dcff8a7a))
* label T1 suspend display errors ([42688da](https://github.com/dstoc/sony-prs/commit/42688da58d033767087787e63995aab8e60c667f))
* log T1 wake timing and power source ([75371a2](https://github.com/dstoc/sony-prs/commit/75371a29cba43a8fc9a16ae1524e620aa2b192fe))
* make T1 suspend mode selectable ([bfb01f6](https://github.com/dstoc/sony-prs/commit/bfb01f6586aaa0328edd8e6aa98d8069d3909b18))
* provide native standby screen ([9f4b849](https://github.com/dstoc/sony-prs/commit/9f4b849def07b82e4adbf7b64403a9fc04b2b72e))
* redraw on T1 menu hold ([356f4aa](https://github.com/dstoc/sony-prs/commit/356f4aaee809ec453a729e8b36f2799cd5ce6200))
* refine T1 native status bar ([c74398f](https://github.com/dstoc/sony-prs/commit/c74398f49c2f9a9f121c8f4f12d6b245a811a572))
* report Wi-Fi service state ([ecf8ee0](https://github.com/dstoc/sony-prs/commit/ecf8ee0246c9550336725fd5f596b6f53fb967c8))
* restart adb after native wake ([d274413](https://github.com/dstoc/sony-prs/commit/d27441371e75f0e55137bd3df96a96e1b7e2b879))
* scaffold PRS-T1 native agent plan ([65834f7](https://github.com/dstoc/sony-prs/commit/65834f7591ffb68eb861f3c1f6f7a122365ab358))
* show live status in native T1 UI ([d8a07f1](https://github.com/dstoc/sony-prs/commit/d8a07f12bfa68d5c885ad920aeffb04e3a12ce39))


### Bug Fixes

* accept legacy T1 touch axes ([0595a25](https://github.com/dstoc/sony-prs/commit/0595a25ea7ba966bd60dac23c651ba41906533fb))
* avoid native wake barrier worker thread ([cfe6f33](https://github.com/dstoc/sony-prs/commit/cfe6f33963ea6d6cfbcf27de48844bb15b77eee8))
* defer adb restart until usb reconnect ([8d7f140](https://github.com/dstoc/sony-prs/commit/8d7f140770a616bd69f04ededae8be749d5b9e1f))
* detach T1 launcher before stopping zygote ([88dd62a](https://github.com/dstoc/sony-prs/commit/88dd62aadd25f333bd6c8efc710d43ca9a6924c1))
* hand off T1 wake events to kernel ([cb5be58](https://github.com/dstoc/sony-prs/commit/cb5be58459bf74ee92dd31b17515309d9cc6cd3c))
* keep status text clear of display target ([134eb54](https://github.com/dstoc/sony-prs/commit/134eb54566abbf22b397922063da7b9eefae138a))
* keep T1 wake events live during barrier wait ([1ede540](https://github.com/dstoc/sony-prs/commit/1ede540556461b7d7b888f1d35ddf80c89414160))
* retain T1 framebuffer mapping across wake ([b4d48bc](https://github.com/dstoc/sony-prs/commit/b4d48bc7e7aba5c13f6a1fd16494eb3872152c86))
* retry T1 framebuffer remap after wake ([b3ce4e0](https://github.com/dstoc/sony-prs/commit/b3ce4e0f5d3c0b787bb1abf89a4b54576d77cd24))
* space native status panels ([27ff390](https://github.com/dstoc/sony-prs/commit/27ff390643b53f0ae040c1921cc86c3f79ba28dd))
* use full EPDC mode for forced redraws ([57176d0](https://github.com/dstoc/sony-prs/commit/57176d086b7fd29d901d4207888b99166aa449e3))
* use T1 EINK standby for native sleep ([8bd157b](https://github.com/dstoc/sony-prs/commit/8bd157b1ab4cc10eec3b1d090df3a459ec7598a3))
* wait for T1 display wake barrier ([b32805e](https://github.com/dstoc/sony-prs/commit/b32805e4637e012f97512ff1c3158937064defb7))


### Performance Improvements

* submit transient refreshes asynchronously ([cb7da59](https://github.com/dstoc/sony-prs/commit/cb7da59e20ca75012fc0f99ab7732c8f6bea577d))
* track pixel damage for native display updates ([b24411d](https://github.com/dstoc/sony-prs/commit/b24411d570b119341fd049938d5c2427a5be201d))
* use DU waveform for fast native updates ([871db20](https://github.com/dstoc/sony-prs/commit/871db20877389a0bbb190a03acc04e9a58276210))
* use partial regions for native ui updates ([e0a2175](https://github.com/dstoc/sony-prs/commit/e0a21753d3328f23516c15967fd59de9da0093fb))

## Changelog

All notable changes to `prs-t1-agent` are documented here by Release Please.
