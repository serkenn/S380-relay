# s380-relay

2台の Sony **RC-S380**（NFC Port-100）リーダ間で、ISO14443 スマートカードの通信を
ネットワーク越しに **APDU レイヤ**で中継する Rust 製ツールです。
[`felica`](https://crates.io/crates/felica) クレートの上に実装しています。

一方のリーダ（**サーバ**）に実カード（JavaCard applet、スマートカード等）を載せ、
もう一方のリーダ（**クライアント**）がスマホに対してカードとして振る舞います。
スマホをクライアントにかざすと、スマホが送るコマンド APDU が TCP でサーバへ中継され、
実カードへ送られ、応答 APDU が返されます。スマホからは実カードと会話しているように見えます。

```
 スマホ ⇢ (Type A / ISO-DEP) ⇢ [client RC-S380] ──APDU を TCP で──▶ [server RC-S380] ⇢ 実カード
          └ ここで ISO-DEP を終端                                  └ ここで ISO-DEP を終端
                                 リンクを渡るのは ISO 7816-4 APDU のみ
```

**自分が所有する（またはテスト許可のある）リーダ・カード**で、研究・CTF・相互接続検証に
使うことを想定した NFC リレー/エミュレーション環境です。

## クロステクノロジ中継（A ⇄ B）

Type A（Type 4）も Type B も、上位の **ISO14443-4（APDU レイヤ）は共通**です。
各面が自分のリーダ上で ISO-DEP を終端し、中継するのは APDU だけなので、
実カードは **Type A でも Type B でも**よく、クライアントは常にスマホへ
**Type A の Type4 カード**として振る舞います。スマホは実カードの技術方式を意識しません。

| 実カード（サーバ側） | スマホへの見え方（クライアント側） | 中継 |
|--------------------|------------------------------|:---:|
| NFC-A, ISO-DEP (Type 4) | NFC-A Type 4 | ✅ |
| NFC-B, ISO14443-4 | NFC-A Type 4 | ✅ |
| NFC-A, 非 ISO-DEP (Type 2 等、MIFARE Ultralight) | — | ❌（APDU レイヤが無い） |

RC-S380 は Type B をエミュレートできませんが、その必要はありません。クライアント側は
常に NFC-A だからです。中継できるのは **ISO14443-4 で会話するカードのみ**です
（ただの Type 2 タグには APDU レイヤがありません）。

### UID / 識別情報

クライアントはスマホへ**合成した Type4 の識別情報**（先頭 `0x08` の 4 バイト UID と
汎用 ATS）を提示し、実カードの UID/ATS/ATQB は再現しません。これは ISO 7816-4 の
APDU 交換（JavaCard applet が関知する部分）には影響せず、リーダが観測する下位層の
識別情報だけが異なります。

## 必要なもの

- Rust 1.88 以降（2024 edition）
- Sony RC-S380 を 2 台（別々のマシンに 1 台ずつ、または 1 台のホストに 2 台）
- リーダへの USB アクセス権（Linux では udev ルールや十分な権限が必要な場合あり）

### Windows: Zadig で WinUSB ドライバを当てる

本ツールは `libusb`（`rusb` クレート経由）でリーダと通信します。Windows では
これを使うためデバイスを **WinUSB** ドライバに紐づける必要があります。RC-S380 は
通常 Sony 純正ドライバ（FeliCa ポートソフト用）を使っているため、そのままでは
`libusb` が開けません。ドライバを置き換えてください。

[Zadig](https://zadig.akeo.ie/) を使用します:

1. RC-S380 を接続して Zadig を起動。
2. **Options → List All Devices** を選び、RC-S380（`SONY RC-S380`、USB ID
   `054C:06C1`）を選択。
3. ターゲットドライバに **WinUSB** を選び **Replace Driver**（または *Install Driver*）。
4. もう 1 台のリーダでも同じ操作を行う。

リーダごとに実施してください。あとで Sony の FeliCa ソフトに戻したい場合は、
デバイスマネージャーで該当デバイスの WinUSB ドライバを削除し、Windows に
純正ドライバを再適用させます。macOS / Linux ではこの作業は不要です。

## ビルド

```sh
cargo build --release
```

## 使い方

バイナリ 1 つに `server` / `client` / `list` のサブコマンド。ログは `RUST_LOG` で制御します。

### 1 台のホストで 2 台使う場合

`list` で接続中のリーダと index を確認できます。

```sh
$ ./target/release/s380-relay list
attached RC-S380 readers:
  index 0  bus 003 addr 003  pid 0x06C1
  index 1  bus 003 addr 005  pid 0x06C1
```

1 台のホストに 2 台ある場合は `--device-index` で各面に別のリーダを割り当てます
（サーバ既定 `0`、クライアント既定 `1`）。2 台を別ホストに繋ぐ場合は各ホストから
1 台しか見えないので既定のままで構いません。

### サーバ（カード側）

実カードをこのリーダに載せてから:

```sh
RUST_LOG=info ./target/release/s380-relay server --listen 0.0.0.0:7878
```

| フラグ | 既定 | 意味 |
|------|------|------|
| `-l, --listen <addr:port>` | `127.0.0.1:7878` | 待ち受けアドレス |
| `--tech <a\|b>` | `a` | 実カードの ISO14443 方式 |
| `-d, --device-index <n>` | `0` | 使用する RC-S380（`list` 参照） |
| `-t, --timeout <ms>` | `1000` | コマンドごとのタイムアウト |

### クライアント（スマホ側）

サーバへ接続して Type4 カードのエミュレーションを開始し、スマホをかざします:

```sh
RUST_LOG=info ./target/release/s380-relay client --connect <server-ip>:7878
```

| フラグ | 既定 | 意味 |
|------|------|------|
| `-c, --connect <addr:port>` | `127.0.0.1:7878` | サーバアドレス |
| `-d, --device-index <n>` | `1` | 使用する RC-S380（`list` 参照） |
| `--no-wtx` | （無効） | 中継前に S(WTX) を送らない |
| `--wtxm <1-59>` | `10` | 待ち時間延長の係数 |
| `-t, --timeout <ms>` | `1000` | コマンドごとのタイムアウト |
| `-w, --window <seconds>` | `1.0` | listen ウィンドウ長 |

各コマンドの中継前にスマホへ `S(WTX)` を送り、ネットワーク往復がスマホの
フレーム待ち時間（FWT）に収まるようにしています。`--wtxm` で調整、`--no-wtx` で無効化できます。

## 仕組み

1. クライアントがサーバへ `get_card` を要求。サーバは実カードを ISO-DEP へ活性化します
   （Type A は anticollision + SEL + `RATS`、Type B は `SENSB` + `ATTRIB`）。ATS/ATQB を返します。
2. クライアントは合成 Type4(NFC-A) カードを提示して listen。スマホがかざすと、
   スマホとの ISO-DEP を自分で終端（`RATS` にローカル応答）し、**コマンド APDU** を再構成します。
3. そのコマンド APDU を TCP でサーバへ中継。サーバは PCD 側 ISO-DEP 状態機械
   （`src/isodep.rs`）で実カードと交換します（ブロック番号トグル、コマンド/レスポンスの
   チェイニング、カードからの `S(WTX)` 対応）。応答 APDU を返します。
4. クライアントは応答を ISO-DEP I-ブロックに包んでスマホへ返します。2 つの ISO-DEP
   セッション（スマホ↔クライアント、サーバ↔カード）は独立で、共有されるのは APDU だけです。
   これにより Type B カードを Type A として見せられます。

## ワイヤプロトコル

TCP 上の改行区切り JSON（`felica-rs` の remote 例に倣っています）:

```jsonc
// client → server
{"type":"get_card"}
// server → client
{"type":"card","tech":"B","info":"5090be4e5b000005e0b381a100"}

// client → server: コマンド APDU を 1 つ中継
{"type":"apdu","data":"00A4040007A0000002471001","timeout_ms":1000}
// server → client
{"type":"apdu","data":"6F..9000"}      // 応答 APDU
{"type":"error","message":"..."}
```

`data` は hex エンコードした ISO 7816-4 APDU です。ISO-DEP のフレーミング
（PCB・CRC・チェイニング・WTX）は各面で処理し、リンクを渡りません。

## 注意・制約

- 中継できるのは ISO14443-4 のカードのみ。ただの Type 2 タグには APDU レイヤがありません。
- エミュレートするカードの UID/ATS は合成値で、実カードのものではありません（上記参照）。
- 中継遅延はネットワークに依存します。`S(WTX)` で時間を稼ぎますが、極端に遅い回線では
  スマホがセッションを切ることがあります。
- コマンド/レスポンスのチェイニング経路は実装済みですが、全ケースを実機検証できていないため
  ベストエフォートです。多くの APDU は 1 ブロックに収まります。
- 1 面につき物理リーダ 1 台のため、サーバは同時に 1 クライアントのみを処理します。

## ライセンス

Apache-2.0 — `LICENSE` を参照。
