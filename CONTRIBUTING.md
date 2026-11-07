# Contributing

MonsterMemoryはAttraqualiaのプロジェクトで、保守担当はhalnosです。APIと永続化形式はalpha段階です。変更前に関連するexamples、テスト、[設計判断](docs/adr/0001-index-architecture.md)を確認してください。

## 開発環境

リポジトリのルートから実行します。`rust-toolchain.toml`はRust 1.94.1とrustfmt／Clippyを指定しています。Rustをインストール済みであればrustupがこの指定を選択します。OSに対応するリンカーが必要です。

## 検証

既存CIは次の書式・lint・テスト・ドキュメント確認を行います。ワークスペースの追加featureは現在`monstermemory/persistence`だけで、排他的featureはありません。以下では検査するfeatureを明示しています。

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features monstermemory/persistence -- -D warnings
cargo test --workspace --features monstermemory/persistence
cargo test --workspace --no-default-features
cargo test --workspace --doc --features monstermemory/persistence
cargo doc -p monstermemory -p monstermemory-core -p monstermemory-macros -p monstermemory-persistence --features monstermemory/persistence --no-deps
cargo xtask manual
cargo xtask manual-check
cargo package --workspace --allow-dirty
```

rustdocの警告をエラーとして検査する場合は、`cargo doc`の前に`RUSTDOCFLAGS=-D warnings`を設定します。

```powershell
$env:RUSTDOCFLAGS = '-D warnings'
cargo doc -p monstermemory -p monstermemory-core -p monstermemory-macros -p monstermemory-persistence --features monstermemory/persistence --no-deps
Remove-Item Env:RUSTDOCFLAGS
```

`cargo package`はローカルのアーカイブ作成・検査であり、crates.ioへアップロードしません。

## ドキュメントと使用例

- READMEと日本語マニュアルの主言語を維持し、外部Rust APIの新規rustdocは英語で書きます。
- `///`は目的・契約・必要なErrors／Panics／Safety、`//!`は入口・モジュールの説明に使います。関連APIはintra-doc linkで参照します。
- 正常に動く使用例はdoctestとして検証します。derive利用例は`monstermemory`側で検証します。
- `docs/manual/src/manual.md`がHTMLマニュアルの編集元です。生成済みHTMLは手で編集せず、`cargo xtask manual`で更新します。
- 内部コメントは選択理由・不変条件・処理順序の制約を補います。各ファイルへ作者表記を繰り返し追加しません。

参考資料は[Rust Book](https://doc.rust-lang.org/book/ch14-02-publishing-to-crates-io.html)、[rustdoc book](https://doc.rust-lang.org/rustdoc/how-to-write-documentation.html)、[Rust API Guidelines](https://rust-lang.github.io/api-guidelines/documentation.html)、[Rust Style Guide](https://doc.rust-lang.org/style-guide/index.html#comments)です。unsafeが必要になった場合の説明方針は[標準ライブラリ開発者向け規約](https://std-dev-guide.rust-lang.org/policy/safety-comments.html)を参考にします。

## 変更の報告

再現方法、変更後の挙動、実行した検証、未確認環境を記載してください。performance変更は同じhardware／seedでbefore／afterを測定し、`BENCHMARK_REPORT.md`へ追記します。公開API、永続化形式、ライセンスの変更は文書修正と区別して説明してください。

現在、製品クレートはunsafeを禁止しています。unsafe導入はbenchmark根拠、局所invariant、Miri／Loom test、`SAFETY.md`更新なしには受け入れません。脆弱性の報告は[SECURITY.md](SECURITY.md)に従ってください。

