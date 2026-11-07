# Security policy

MonsterMemoryはalpha段階です。安定版と同等のセキュリティ保守期間、修正期限、互換性は表明していません。保守担当はhalnos（Attraqualia）です。

## 脆弱性の報告

公開リポジトリの **Security → Advisories → Report a vulnerability** が利用できる場合は、その非公開報告機能を使ってください。報告が非公開になることを確認してから、影響するversion、再現手順、最小限のダミーデータ、想定される影響を送ってください。

そのボタンが表示されない場合は、機密情報や攻撃手順を載せずに、Issueで保守担当へ非公開の報告経路の案内だけを依頼してください。実データ、秘密鍵、token、個人情報は公開しないでください。未確認のメールアドレスや代替窓口は掲載していません。

このリポジトリではprivate vulnerability reportingを使用しています。非公開の報告はSecurityタブから送信してください。

## 永続化入力と制約

container readerはmagic、format version、保存fingerprint、payload length、configured size limit、CRC32を検査し、checkpointではfile lengthの一致も検査します。低レベルの`decode_table`だけを呼ぶ場合にはcontainerの検査は付きません。

- CRC32は偶発的な破損を検出するもので、敵対的な改ざんの認証ではありません。
- payloadの符号化サイズの制限は、Serdeのdecode時に確保するobjectのメモリ量の制限ではありません。
- database fingerprintはrecordの全field定義を含まず、同じ型名でのschema変更を確実には検出しません。
- checkpoint／WAL pathは単一のdatabaseインスタンスが排他的に管理してください。プロセス間のlockはありません。
- 不完全なWAL末尾は復旧時に読み飛ばしますが、自動修復しません。異常終了後に復旧した場合は、同じpathへのcheckpointを成功させてから追加writeを行ってください。

engineが生成するエラーはrecord本文を自動で含めませんが、利用側のserializer、validator、ログ処理が機密値を出力しないことまでは保証しません。詳しくは[SAFETY.md](SAFETY.md)を参照してください。

