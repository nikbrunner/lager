# Changelog

## [1.0.0](https://github.com/nikbrunner/lager/compare/v0.1.1...v1.0.0) (2026-11-23)


### ⚠ BREAKING CHANGES

* list and ls print the status table and JSON schema 2 in place of the tab-separated declaration lines and schema 1. The library's Command::List and ListArgs are Command::Status and StatusArgs.
* start the interactive screen with lager tui

### Features

* add a repository from the inventory by reference ([36a4d8a](https://github.com/nikbrunner/lager/commit/36a4d8a5774259b7c5593786d66931a62f1e7c20))
* add marks, the grouped action menu, copy and session problems ([1e9b89e](https://github.com/nikbrunner/lager/commit/1e9b89e63e27a3531eabb4fe1357ac61a83789ce))
* add two new skills ([08ab33d](https://github.com/nikbrunner/lager/commit/08ab33d8437d3ee24d1c3d07180165ee23a587f0))
* add worktree list, add and remove commands ([0f1157a](https://github.com/nikbrunner/lager/commit/0f1157ad648e110a0a33d630b654a295dcaab81a))
* archive and change visibility of GitHub repositories ([237ad19](https://github.com/nikbrunner/lager/commit/237ad191f6b17f02347ac95e9ff70a6d212b7830))
* bind every inventory action through the configurable key maps ([349f618](https://github.com/nikbrunner/lager/commit/349f6188cd713f021be97b45882b62e8b7641dc7))
* bring the inventory screen onto the canvas look ([8e5134e](https://github.com/nikbrunner/lager/commit/8e5134e32cb1cd93adc1190a6062d131aa178bb5))
* clone from the inventory with a native terminal handoff ([b538a98](https://github.com/nikbrunner/lager/commit/b538a987cf80fe79318aa92bc4f1d165d88edfd3))
* close the session of a checkout that is gone ([4bfcd0d](https://github.com/nikbrunner/lager/commit/4bfcd0d2ed823881e7a995d8e1a8efc0a7f060e3))
* create a GitHub repository with lager add --create ([7815e9c](https://github.com/nikbrunner/lager/commit/7815e9c06fdf5c34a5924c988ccf7d6f2d966578))
* delete a repository everywhere with lager remove --remote ([c373e64](https://github.com/nikbrunner/lager/commit/c373e64be6f7715215cac4619eac7076d42629af))
* discover provider repositories in the inventory with lager tui --remote ([0e8fb94](https://github.com/nikbrunner/lager/commit/0e8fb94ab0178cf6068195df2a6d9ccae146fb1b))
* drop the remote column from the inventory grid ([1743e1e](https://github.com/nikbrunner/lager/commit/1743e1ef34d1a4a417be6a8bcada6a05526bcec2))
* filter the inventory by changes, owner, registration and session ([#34](https://github.com/nikbrunner/lager/issues/34)) ([f0709f4](https://github.com/nikbrunner/lager/commit/f0709f49032d3a16c4b6aeb16c9bb3e6a3166eb6))
* give the branch its own column and move registration last ([897a29c](https://github.com/nikbrunner/lager/commit/897a29c9e89348c8c158f65729fe0816ba6705e7))
* keep the hidden count and the blocked outcome on screen ([3fd3921](https://github.com/nikbrunner/lager/commit/3fd3921807cb5f5a0f11da06934770b057b1ac23))
* keep the hint bar empty while a popup is open ([0a7579c](https://github.com/nikbrunner/lager/commit/0a7579c07051a368b616907c1a8e8f8bead61171))
* list a missing root as empty and record the 1.0 acceptance evidence ([90c91fe](https://github.com/nikbrunner/lager/commit/90c91fe37fe8d6cfedd6278808c2ebebef071253))
* list a wildcard's members, not the wildcard itself ([9fe18f8](https://github.com/nikbrunner/lager/commit/9fe18f8486127dd8f9b21d005af14098d3ce7849))
* list the actions without a key on one line in help ([dedfeb9](https://github.com/nikbrunner/lager/commit/dedfeb97647a5762b75a1418d7516096348180bb))
* manage worktrees from the inventory with a popup on w ([edf9c86](https://github.com/nikbrunner/lager/commit/edf9c86fd2ea86c7befe16a7c0a88dd78a50b0bc))
* open a checkout as a terminal session with lager session ([1f1c434](https://github.com/nikbrunner/lager/commit/1f1c434390a453fdd1719b8b9a51bd4d89ecfdea))
* open a repository's session from the inventory with s ([4c2b494](https://github.com/nikbrunner/lager/commit/4c2b4946ebfe6e02f892c6667e02af7758433f78))
* open and close sessions from the worktrees popup ([17af56e](https://github.com/nikbrunner/lager/commit/17af56e18a9b70a78936eff5dbdddb82296f275f))
* open lazygit for a repository from the inventory ([#33](https://github.com/nikbrunner/lager/issues/33)) ([fa2a88f](https://github.com/nikbrunner/lager/commit/fa2a88fd1d527c2d7508a9ddf027a9ca3b4ff8fe))
* open or close a worktree's session with the worktree commands ([9f51a56](https://github.com/nikbrunner/lager/commit/9f51a562c935f37542aa5ab7f0a54e2aa6fa25b3))
* pick the checkout for session open and close ([cafcec7](https://github.com/nikbrunner/lager/commit/cafcec7517d857efaab9ae7813f7e0fb97b3fd44))
* print the agent guide and install the skill with lager skill ([19a37bb](https://github.com/nikbrunner/lager/commit/19a37bbd8a6dcc9845473da1a5a4cf53353538e3))
* print the inventory rows with lager status ([2eafca2](https://github.com/nikbrunner/lager/commit/2eafca2f4be7780ce10c261ca14f8f49fcceef9d))
* publish a local repository with lager add --create --from ([f509dd2](https://github.com/nikbrunner/lager/commit/f509dd2d64ade3916380089af8b843dd930108f5))
* rank inventory search like fzf ([e0da1fa](https://github.com/nikbrunner/lager/commit/e0da1fae028c426175b9c49a5b5739ba1de487b7))
* register and unregister from the inventory ([2cc3ce0](https://github.com/nikbrunner/lager/commit/2cc3ce01ca77b0233cae42a40c1a55efd156e821))
* remove a checkout or worktree only when nothing is provably lost ([242a014](https://github.com/nikbrunner/lager/commit/242a014837e78c4f209ec6c81436303a573f67f3))
* remove checkouts from the inventory with confirmation and batches ([3577975](https://github.com/nikbrunner/lager/commit/35779758fdc8f405015b6ec4417c68980366df76))
* rename a GitHub repository with lager rename ([82a38f0](https://github.com/nikbrunner/lager/commit/82a38f00ac60f610a6114cdfacfbe94b8927d6cc))
* report ensure clone progress ([9a8461a](https://github.com/nikbrunner/lager/commit/9a8461a08efb521045b448a3ce0f0421170fe32d))
* run the GitHub actions from the inventory ([f752ae1](https://github.com/nikbrunner/lager/commit/f752ae14445bacd9d7aa505e57480c255db60a3e))
* show a missing repository without remote discovery ([ac50395](https://github.com/nikbrunner/lager/commit/ac503952e58d6aca714c2a5efe60cdb38c714d4e))
* show batch outcomes and quit choices without explanations ([706fe0b](https://github.com/nikbrunner/lager/commit/706fe0bb9ad458ca29e994a7e51c1b915d87474d))
* show each repository's remote state in five inventory columns ([7864d3b](https://github.com/nikbrunner/lager/commit/7864d3b294e2c824fb79f628716634b799097000))
* show only the problems in the Problems popup ([ef4c688](https://github.com/nikbrunner/lager/commit/ef4c6886e2b7f4d200789278a3c4a732b8f57c84))
* show only the repository facts in the Inspect popup ([8bea1aa](https://github.com/nikbrunner/lager/commit/8bea1aa43b62ca7bcc46307a4d12631c8418c3ca))
* show the add form and previews without explanations ([e4002b0](https://github.com/nikbrunner/lager/commit/e4002b07a6aca0c83a099d6b608365744193bb5f))
* show the GitHub popups without explanations ([f4f085f](https://github.com/nikbrunner/lager/commit/f4f085f9dd05865ba1d434a4270cce98c2b8cf19))
* show the menu entries without explanations ([2c13d18](https://github.com/nikbrunner/lager/commit/2c13d18d6e5938a891f33bc0aaaa0a87fbae4fbb))
* show the owner column and order rows by owner ([#31](https://github.com/nikbrunner/lager/issues/31)) ([b6334b3](https://github.com/nikbrunner/lager/commit/b6334b38a3b1a6b5a46fe25390df1f7b32cf7969))
* show the remote discovery menu without explanations ([c861a62](https://github.com/nikbrunner/lager/commit/c861a62544afa42fafcd6fa491a0a7947c02a27d))
* show the removal confirmation without explanations ([c437164](https://github.com/nikbrunner/lager/commit/c4371645e33dda23d9d2c2b577522629baa445d6))
* show the worktrees popup without explanations ([3f7f332](https://github.com/nikbrunner/lager/commit/3f7f332adbda36d388695aa6f0d2f71364c4132d))
* start the interactive screen with lager tui ([3129202](https://github.com/nikbrunner/lager/commit/312920218f2d1c51fbb122f5a1ce740659134ec2))
* toggle the dirty filter with Ctrl+d ([38a79dd](https://github.com/nikbrunner/lager/commit/38a79dd236e6bb6d0e1971fa8ac99339ec6aca65))


### Bug Fixes

* delete and rename only the exact GitHub name and read origin as written ([b31b032](https://github.com/nikbrunner/lager/commit/b31b032ed87c9f7049241403c157fd4538b9639a))
* enforce repository management safety contracts ([09ff5ba](https://github.com/nikbrunner/lager/commit/09ff5baed305412264cfb8a8cb4cbfdb1bbf03e7))
* fail without a terminal where a command would open a picker ([0df4907](https://github.com/nikbrunner/lager/commit/0df4907908cf3d4f20bd1ba99c52bf26dcfba850))
* name gh's exit status when it fails without a message ([2148b9b](https://github.com/nikbrunner/lager/commit/2148b9b310db694fbef75a5d893b236865884ed9))
* never take optional Git locks in read-only probes ([60ea4d8](https://github.com/nikbrunner/lager/commit/60ea4d83d1dda283f0a5cced184f7856d6e5c1fd))
* open the remove picker without inspecting every checkout ([63f0711](https://github.com/nikbrunner/lager/commit/63f0711ce2e4ec384d7f6eb25f719424de708016))
* report cloned repository location ([47b8c2a](https://github.com/nikbrunner/lager/commit/47b8c2a8a61f96ca9168c27bea0c24e44f4e67c5))
* report cloned repository location ([8b9df60](https://github.com/nikbrunner/lager/commit/8b9df60f54787cdd89dd723d69dc8c0321e1d388))
* resolve warehouse clippy warnings ([6c83c83](https://github.com/nikbrunner/lager/commit/6c83c83657c1f1722a5f300b053d528010eff138))
* restore the terminal first and stop background work within 2 s ([c47eb9f](https://github.com/nikbrunner/lager/commit/c47eb9f5a0dd024fc7b1e32950f2f027c4081e13))
* select repositories for hook runs ([d39fd60](https://github.com/nikbrunner/lager/commit/d39fd603d35fadb5f43f466199cd66cbc37a17b7))
* show exact values whole and bring the docs in line with the binary ([b849eb4](https://github.com/nikbrunner/lager/commit/b849eb4ba94ebc5e28a1f9e7fdd31e17725c35ba))


### Performance Improvements

* build the inventory in linear time ([4c94544](https://github.com/nikbrunner/lager/commit/4c945447e134b2530a887e7cc6d52698c5516316))

## [0.1.1](https://github.com/nikbrunner/lager/compare/v0.1.0...v0.1.1) (2026-09-13)


### Bug Fixes

* remove invalid GitHub repository field ([9292c1c](https://github.com/nikbrunner/lager/commit/9292c1c704782b132ded30f1742a4eaee2a800e7))

## 0.1.0 (2026-09-13)


### Features

* create Lager CLI ([d7af42f](https://github.com/nikbrunner/lager/commit/d7af42f94bfe14d86a26c32dfb417685affa4b7a))


### Bug Fixes

* preserve early fzf cancellation ([1807aa2](https://github.com/nikbrunner/lager/commit/1807aa2633ecce4abff674812e89a62cca794b48))
