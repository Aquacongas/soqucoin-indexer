# SOQ.MARKET on-chain protocol specification

This specification describes the standalone Soqucoin Indexer release represented by source snapshot `b7d8fc9`. Its application rules define compatible asset state over blocks supplied by Soqucoin Core. Core determines chain validity and the active branch. Wallet preflight, rendering, database migration, and local marketplace projections are identified separately below.

## Contents

1. [Inventory and versions](#1-inventory-and-versions)
2. [Chain inputs and trust](#2-chain-inputs-and-trust)
3. [Binary carrier and operations](#3-binary-carrier-and-operations)
4. [Ordered processing, ownership and fees](#4-ordered-processing-ownership-and-fees)
5. [SQR-20](#5-sqr-20)
6. [SOQ-721](#6-soq-721)
7. [Shared marketplace operations](#7-shared-marketplace-operations)
8. [SOQ-FINGERPRINT](#8-soq-fingerprint)
9. [SOQ-MAPS](#9-soq-maps)
10. [FATCAT](#10-fatcat)
11. [Confirmations and mempool](#11-confirmations-and-mempool)
12. [Rollback and replay](#12-rollback-and-replay)
13. [Verification vectors](#13-verification-vectors)
14. [Limits and independent implementation](#14-limits-and-independent-implementation)
15. [Source and test references](#15-source-and-test-references)

## 1. Inventory and versions

| Protocol family | Implemented variants | SQMK operation codes (decimal) |
| --- | --- | --- |
| SQR-20 | Integer accounting, application version 2 | 2, 3 and shared operations |
| SOQ-721 | Version 1, standalone and collection-bound | 1, 6, 7 and shared operations |
| SOQ-FINGERPRINT | Canonical seed v1, renderer `soq-fingerprint-v3` | 5 (internally named MapRequest), shared operations |
| SOQ-MAPS | Canonical seed v1, renderer `soq-maps-quadtree-2` | 8, shared operations |
| FATCAT | Historical claim-based draw v1 and direct draw v2 | 12–16, shared operations after reveal |

All five families use one wire version, SQMK v1. The numbers in renderer names, draw metadata and application constants do not occupy additional wire fields. Shared Transfer, Listing, Settle and Cancel are described in §7 and §4. The library also implements the separate SVG renderer `soq-maps-1`, covered in §9. The confirmed ledger invokes neither that renderer nor the fingerprint preview renderer.

## 2. Chain inputs and trust

Use an independently validated Soqucoin Core active chain. Read the block height, displayed block hash, parent hash, time and ordered transactions, including displayed txids, ordered input outpoints or coinbase marker, and ordered output indices, values and raw locking scripts. The release obtains these from `getblockchaininfo`, `getblockhash` and verbosity-2 `getblock`. Its CLI requires an unpruned node with working transaction indexing, binds the selected network genesis, and starts empty stores at genesis. Transaction detail and mempool observation also use `getrawtransaction`.

Core is responsible for block and transaction consensus, including chain selection, AuxPoW, amounts, transaction signatures and witness validation, including applicable post-quantum signature rules, and coinbase spend maturity. The indexer trusts those decisions and does not independently validate proofs, signatures, script execution, transaction IDs or block hashes against serialized data. It checks continuity, identities as reported by RPC, coinbase position, and existence and unspent status of indexed prevouts. The local address helper constructs a v1 32-byte program from SHA-256 of a 1312-byte ML-DSA-44 public key (rejecting a first byte of 255 in that helper). Its Bech32m address encoding uses version 1 and HRP `sq` or `ssq`. The asset interpreter checks raw script shape where specified below.

One SOQ is 100,000,000 atoms. Parse decimal coin values exactly as unsigned decimal with a whole part and up to eight fractional digits. Atoms = whole × 100,000,000 + right-zero-padded fraction. The implementation rejects signed and exponent forms and values above 2⁶³−1 atoms. Application quantities are separate from SOQ and use whole units.

## 3. Binary carrier and operations

Inspect output scripts whose first byte is `6a` (OP_RETURN). For an application operation there must be exactly one such output, including unrelated or malformed OP_RETURN outputs in the count. Its entire script is `6a L payload` for lengths through 75, or `6a 4c L payload` for lengths 76–80. L is one byte. Trailing data, alternative pushes, unknown versions and unknown operation codes fail. The generic parser does not restrict carrier atom value. FATCAT layouts below impose zero for some carriers.

The payload starts with `53 51 4d 4b 01` at offsets 0–4. Offset 5 is the operation byte. All offsets below are zero-based payload offsets. Fixed integers are unsigned little-endian (LE) unless specified. A destination is a one-byte output index. A referenced txid uses bytes decoded from the displayed 64-digit hex string in its displayed order. Ordinary transaction outpoints in raw Core serialization use the reverse of displayed order. Metadata and offer commitments are uninterpreted 32-byte values at decode time. There are no on-chain UTF-8 strings in these formats. The ticker uses ASCII with zero padding. A suffix listed as optional must be completely present or absent.

| Code hex | Operation | Payload length | Fields as offset:length and meaning |
| --- | --- | --- | --- |
| 01 | NFT | 39 | 6:32 metadata, 38:1 destination |
| 02 | Deploy | 62 or 78 | 6:32 metadata, 38:8 cap, 46:8 mint limit, 54:8 ticker, 62:16 optional name key |
| 03 | Mint | 47 | 6:32 token ID, 38:8 quantity, 46:1 destination |
| 04 | Transfer | 39 + 9N | 6:32 asset ID, 38:1 count N (1–4), then N pairs at 39 + 9i, each destination byte then 8-byte quantity |
| 05 | Fingerprint request | 7 | 6:1 destination |
| 06 | Collection | 42 or 58 | 6:32 metadata, 38:4 maximum, 42:16 optional name key |
| 07 | Collection NFT | 71 | 6:32 collection ID, 38:32 metadata, 70:1 destination |
| 08 | SOQ-MAPS request | 7 | 6:1 destination |
| 09 | Listing receipt | 38 | 6:32 offer commitment |
| 0a | Settle | 54 | 6:32 asset ID, 38:8 quantity, 46:8 price in atoms |
| 0b | Cancel | 46 | 6:32 asset ID, 38:8 quantity |
| 0c | FATCAT single claim | 9 | 6:2 serial (1–1006), 8:1 destination |
| 0d | FATCAT claim mint | 7 | 6:1 destination |
| 0e | FATCAT pool mint | 7 | 6:1 destination |
| 0f | FATCAT pool issue | 9 | 6:2 first serial, 8:1 count (1–64), range must fit 1–1006 |
| 10 | FATCAT direct request | 7 | 6:1 destination |

Anchors, change, fees and seller payouts are ordinary transaction outputs. Destination indices refer to this complete output array, including the carrier and change. Decode does not check destination existence, fees, ticker semantics, or the supply and ownership rules.

## 4. Ordered processing, ownership and fees

Process by ascending height and by the actual position in each block. There is at most one application operation per transaction. Apply each spend before evaluating the operation. Unless coinbase, any transaction with an OP_RETURN candidate is attempted. Other transactions and coinbases still affect the base index. Apply finalizers after all transactions in the order fingerprint, SOQ-MAPS, FATCAT.

```text
for block in active_chain_order:
    start atomic update and register block
    for transaction in block order:
        register transaction
        mark each non-coinbase input spent; require existing unspent prevout
        register outputs in array order; require declared index equals array index
        if non-coinbase and any output starts with 6a:
            checkpoint application writes
            decode sole carrier; validate fees and creation names; apply operation
            on failure: revert application writes and record invalid operation
        project matching locally registered trades if applicable
    finalize fingerprint requests, then SOQ-MAPS, then FATCAT
    commit checkpoint
```

The shipping CLI groups up to four contiguous blocks in an atomic window. A repeated current tip with identical height and hash is a no-op. Data-integrity errors outside operation savepoints and finalizer errors fail the entire window. A rejected operation leaves Core's transaction, including consumption of its inputs, in place. An ordinary spend or an invalid operation can destroy an asset balance. There is no automatic carry-forward.

An asset note binds a single asset and a positive quantity at most 2⁶³−1 to an outpoint. A destination must have positive value and exactly 34 script bytes `51 20 <32 bytes>`. Fungible token anchors must have exactly 1,000,000 atoms (0.01 SOQ). A note is live while the underlying output is unspent. The owner is the locking script. When an operation requires a creator script, it takes the first input's prevout script, requires that input be spent by this transaction, and requires the same 34-byte witness shape. SHA-256 denotes a single digest unless specified. Lowercase hex expresses digests in metadata.

**Transfer (04).** Sum notes of the referenced asset on inputs already marked spent by this transaction. Require a positive sum equal to the checked sum of all allocations. Require distinct destination indices and apply the note constraints to each allocation. Validate the transfer fee first. If any allocation fails, none is credited. Other assets spent by the same transaction receive no replacement notes. A one-unit asset necessarily moves in a single positive allocation of one. A transfer may return units to the same script.

### Platform payments

The fixed recipient is `sq1peq2hd53huwgmxy0ezhj5nzrufvxqax3mtdr752qrfmsnvfw6vzmqe0vwjs`. Compare raw script bytes with `5120c81576d237e391b311f915e549887c4b0c0e9a3b5b47ea28034ee13625da60b6`. Multiply the following SOQ amounts by 100,000,000 to obtain atoms.

| Operation | Base fee (SOQ) |
| --- | ---: |
| Standalone or collection NFT | 5,000 |
| Token deploy | 10,000 |
| Token mint | 1 |
| Collection create | 1,000 |
| Fingerprint or SOQ-MAPS request | 7,500 |
| Transfer or listing receipt | 1 |
| FATCAT payment | 2,000 |
| Cancel | 0 |
| Settle | See §7 |

For NFT, deploy, mint, collection, generated-art requests and listing, try full payment first. This requires exactly one recipient-script output of the exact amount anywhere in the array. If this fails, test the output immediately following the carrier as the exact payment and reject any additional output to that script. Transfer always uses the following-carrier placement, subject to the historical exception below. FATCAT has separate fixed layouts and no discount. Fees are application outputs, separate from Core miner fees.

At spending height H the default discount owner is the first input's prevout script. It qualifies for the highest tier for which it has an unspent note of the NFT below with note outpoint height ≤ H−6. Evaluate after marking this transaction's spends and after all earlier transactions in the block. In mempool-aware checks, exclude notes spent by observed mempool inputs. Any tier from {0, 25, 50, 75} at or below eligibility is allowed. For tier d and base B, the authorized amount is ceil(B × (100−d) / 100), calculated with widened integers.

| Discount NFT ID | Tier (%) |
| --- | ---: |
| `f2f8ae134a5a68f6e62815134b4c2890dc2c068e30712c8c7d14a8bc5bf64817` | 75 |
| `eeb9fa1c6c7019b04a4429a30f6f6a969f98e983fda196ca348399bdb4a63186` | 50 |
| `3c598a90b52038ff4f7432ebee970bb5b48e48da11706b854d716c123f0ed772` | 25 |

Historical rules use the indexed block-0 hash `0d828600816cbd7c23789660b53f90cb6ec7ff85540698e13845eb2d2f0486a8` as the mainnet predicate. On that chain, discounts are disabled for H < 56,381. For H < 56,250, NFT, deploy and generated-art request fees instead require the checked sum of all recipient outputs to equal the full fee. Token mint and collection creation bypass the fee check. Listing still uses the normal full-payment path. Historical Transfer first tries the normal full 1 SOQ transfer fee; if that fails, it requires exactly 50,000,000 atoms to the recipient immediately after the carrier, without checking for other recipient outputs.

## 5. SQR-20

**Deployment.** The deployment txid is the ID. Require 1 ≤ limit ≤ cap ≤ 2⁶³−1. There is no initial issuance and no deployer mint privilege. The ticker has 1–8 ASCII characters from A–Z and 0–9, followed by zero bytes through field width 8. Confirmed tickers are unique (the lookup is ASCII case-insensitive). Token and collection names each have their own namespace. The first accepted claim in chain order reserves the key. Validation uses the 16 bytes supplied, including any possible all-zero value. It has no name preimage to verify.

**Name-key convention.** For user-visible names, trim using Rust string whitespace semantics (Unicode White_Space), reject an empty trimmed result and reject more than 100 Unicode scalar values in the original string. Apply `unicase` 2.10.0 full case folding to the trimmed string (default, non-Turkic, without NFC/NFKC normalization), UTF-8 encode, and take the first 16 bytes of SHA-256. The hundred-character and folding rules apply to the helper. The ledger enforces byte-key uniqueness. For example, `Test Token` has key `7b274e01b7610268ddcb0bd5b1c3f2b9`.

**Historical names.** An absent key is accepted only with the §4 mainnet genesis and creation height < 56,330. The following exact metadata values resolve to name keys as above. Other eligible unnamed records reserve no key. Opening an existing writable store also seeds these aliases for matching catalog entries at eligible heights, ignoring name/txid conflicts. That seeding query matches metadata even on records already carrying an explicit key, whose existing claim is retained by the uniqueness constraint.

| Namespace | Metadata hex | Historical name |
| --- | --- | --- |
| Token | `68541a3511a2f81582b5a661e174502c7f00d4a6ccd828f4bc2fbf7ed6f093a7` | SoqDoge |
| Collection | `f5cf0292ab0886c08f58ba0a883231efb5594322c6176e2bbb1927e465f178c1` | SOQ.MARKET |

**Mint.** Anyone whose spend is accepted by Core may mint against an already accepted token. Let M be the sum of all accepted mints for that ID. Require 1 ≤ amount ≤ limit and checked M + amount ≤ cap. Credit one note at the specified destination and add amount to M atomically. Reject NFT IDs, unknown deployments, zero quantities, exhausted caps and incorrect anchors. Minting can occur later in the deployment block. Spending or destroying units does not reopen the cap.

**Accounting.** All serialized cap, limit, mint and allocation amounts are whole token units. For asset A and script S, balance = sum of quantities of unspent notes of A with locking script S. Circulation = sum of balances over all scripts. At all valid checkpoints 0 ≤ circulation ≤ M ≤ cap. A valid transfer, settlement or cancellation conserves the units of its referenced asset. Other spends remove consumed units from circulation. The API renders quantities as decimal strings in relevant views and may limit result counts.

**Integer migration.** SQMK wire version stays 1 and the deploy parser accepts only the 62- and 78-byte payloads. The obsolete precision-byte layouts are rejected, including a precision byte of zero. Database startup first requires the exact ledger version string `sqr20-1-soq721-1-fingerprint-1-fees-1-compact-1` or an empty unversioned block catalog. If the assets schema has a `decimals` column, a transaction removes token deployment/mint/note-producing events selected by their existing records, deletes listings for those tokens (and dependent trades), deletes token assets (and dependent mint and note records), and drops the column. It keeps base blocks, transactions, outputs and other asset types. It does not rescale quantities, reparse old blocks, rewind the tip, or clean every historical receipt/error. A migrated checkpoint may therefore differ from a fresh replay. This is a store upgrade, not a height-gated rule.

## 6. SOQ-721

A standalone NFT operation creates one NFT under its mint txid, with cap and mint limit one and a single one-unit note. A collection operation creates a collection under its creation txid, with metadata, positive u32 maximum and the creator's first-input witness script. The shared name and fee rules apply. The collection itself creates no NFT note and there is no collection ownership transfer operation.

Collection mint requires an existing collection and the same first-input script as the collection creator. The count of prior accepted member mints must be below maximum. Credit one NFT, under the mint's own txid, and a membership relation to the collection. This count includes members later destroyed. There is no wire serial, separate token number, or metadata uniqueness check. The same commitment can occur in multiple NFT transactions. Metadata is exposed as a SHA-256 commitment, but the ledger does not fetch a preimage, define its serialization, or verify media.

Ownership starts at the mint destination and is resolved by the current live note. A rightful spend under Core rules can transfer, settle, cancel or destroy the note under the common rules. Spending alone moves no NFT. Collection authorization is independent of member ownership. Generated assets use the same one-unit note model. FATCAT is exposed as SOQ-721 with the virtual collection `fatcats` and has no collection-creation transaction. Fingerprints and maps have distinct internal asset kinds; they can use the same full-lot operations.

## 7. Shared marketplace operations

Listing receipt (09) records the carrier's 32-byte offer field and first input's spent script after fee validation. A receipt is keyed by transaction, so another transaction may pay a receipt for the same offer and script. Confirmed indexing neither reconstructs the offer nor verifies any off-chain data or signature from that digest. Local listing registration receives pre-existing terms and offer data, and reserves one listing per anchor. The standalone release defines no canonical signed offer encoding or offer hash preimage from chain data.

Cancel and Settle both require exactly one input bearing an asset note across all input assets. It must be input 0 and the note's ID and exact quantity must match the payload. Other, non-note inputs are permitted. Let A be the input anchor's coin value in atoms, and S its script.

- **Cancel (0b)** requires output 0 to use S and be a valid note destination for the same asset and quantity. No output may use the fee recipient script. No registered listing is required.
- **Settle (0a)** requires P > 0 (price in atoms), seller payout output 0 with S and buyer output 1 with a different script that meets the note rules. Calculate seller fee s = checked(A + P) − payout value, with checked subtraction. Let F be the value of the output immediately following the carrier. Buyer fee b = F − s, with checked subtraction. For a token the base fee for both sides is ceil(P × 50 / 10,000), and P must exceed this fee. For any other asset the base seller fee is zero and base buyer fee is 100,000,000 atoms. Each actual fee can equal its base or match a permitted discount. Seller eligibility uses input 0. Buyer eligibility for a discounted fee uses input 1, which must have the same script as output 1 to qualify. Require the exact fee output immediately after the carrier to the platform with no other platform-script output, except seller payout output 0 is excluded from the uniqueness scan. Credit the note to output 1.

Settlement on chain is valid without an existing local listing or matching receipt. Preflight requires a registered listing matching outpoint, ID, quantity, price, payout and seller script and a matching paid receipt. Preflight also rejects Transfer of a locally listed lot until revoked. These checks are local. A confirmed trade projection is written only if a matching listing exists locally and the accepted transaction creates the expected buyer note at output 1. The historical transfer with one allocation at output 1 can also match such a listing by its payout and lot without a price field. Local order books, last prices, volume and trade histories therefore cannot be uniquely reproduced from the chain.

## 8. SOQ-FINGERPRINT

Operation 05 requests a fingerprint. Require the map fee, creator script and a funded v1 destination. The confirmed path applies no input-age check for this operation. Each request is independent. There is no uniqueness check on creators or images.

After each block at tip T ≥ 10, consider unrevealed requests FIFO by payment height P and transaction position. Let next be one more than the largest already revealed fingerprint entropy height, or zero if none. For the first request compute E = max(next, P+5). If E > T−5, stop; otherwise use the active hash at E, generate and credit this request, and repeat with the updated next. No separate future assignment is persisted. Each revealed fingerprint has unique entropy height and block hash within this family. Its asset ID is the request txid and the one-unit note is created at the original destination even if it has already been spent. Such a note is immediately non-live. An anchor spend before reveal cannot transfer an as-yet nonexistent note.

Canonical seed formula, with literal ASCII and NUL, and no separators beyond those shown:

```text
SHA256("soq-fingerprint-v1/canonical\0" ||
       creator_script[34] || displayed_txid_bytes[32] ||
       LE32(destination) || displayed_entropy_hash_bytes[32])
```

Height is metadata only. The low-level renderer accepts a u32 vout, while the wire request uses a u8. The creator script comes from the first input, while the output script determines ownership. Select loop for seed[0] mod 5 in {0,1}, whorl for {2,3}, arch for {4}. The renderer label is `soq-fingerprint-v3`. The non-ledger preview seed is SHA256(`soq-fingerprint-v1/preview\0` || nonce[32] || optional raw creator bytes).

### Fingerprint raster algorithm

Use binary64 with the operation order below, no fused arithmetic, and `libm` 0.2.16 sin/sincos for byte-accurate rendering. Vectors are pairs, dot and length use ordinary component products and square root, unit(v) = v × (1/max(length(v), 1e−9)), normal(x,y) = (−y,x), and complex multiplication uses the usual real and imaginary products. Let a(i) = seed[i]/255.0 and s(i) = a(i)×2−1.

| Field | Definition |
| --- | --- |
| axes, rotation, shift | (163+s(7)×22,204+s(8)×18), (cos,sin) from sincos(s(5)×0.32), (s(19)×13,s(20)×12) |
| handedness h | +1 if seed[6] even, else −1 |
| crest, bend, arch rise | s(9)×58, 0.35+a(10)×0.43, and 0.38+a(27)×0.39 for arch or 0 otherwise |
| boundary b, phase f | (s(11),s(12),s(21),s(22)), (a(23)×6.28,a(24)×6.28,a(25)×6.28) |
| period, width, junction rate | 8.8+a(1)×2.5, 1.35+a(2)×0.52, 28+seed[26] mod 40 |
| cores and deltas | Let c=(s(3)×47,−49+s(4)×33). Arch has neither. Loop uses core c and delta c+(78+s(13)×27,98+s(14)×28). Whorl uses c and c+(s(13)×37,54+s(14)×22), with deltas (−113+s(15)×29,62+s(16)×37) and (114+s(17)×30,75+s(18)×39) |

Local coordinates q are (p−(256,256)−shift) complex-multiplied by conjugate(rotation), then q.x multiplied by h. A point is singular within distance d if its local position is strictly closer than d to any core or delta. Boundary margin is edge−r, where:

```text
y = q.y / axes.y
x = (q.x - 14*y*y - b[0]*19*y) / (axes.x*(1-(0.12+b[1]*0.10)*y))
r = length((x,y)); u = x/max(r,0.01); v = y/max(r,0.01)
edge = 1 + 0.085*b[2]*u*(1-4*v*v) + 0.075*b[3]*(1-8*u*u*v*v)
         + 0.032*(u*u-v*v)*(2*u*v)
```

Inside means margin > 0. For direction, use local q, x=(q.x−crest)/115, z=(q.y+29)/104, arch=arch_rise/(1+z×z). Angle B=(bend+arch)×x/(1+0.55×x×x)+0.10×sin(q.y×0.017+q.x×0.011+f[0])+0.055×sin(q.x×0.039−q.y×0.023+f[1]). Start D=(cos(2B),sin(2B)) from one sincos. Complex-multiply D successively by unit(q−core) for every core, then by conjugate(unit(q−delta)) for every delta. Form (sqrt(max(0,0.5×(1+D.x)))×h, copysign(sqrt(max(0,0.5×(1−D.x))),D.y)), then complex-multiply by rotation. Orient a direction against reference t by multiplying by copysign(1,dot(direction,t)). For spacing and thickness below use canvas p, with no local transform.

```text
spacing(p) = period + 1.15*sin(p.x*0.021-p.y*0.013+f[1])
                    + 0.55*sin(p.y*0.041+f[2])
thickness(p,jitter) = width + jitter + 0.25*sin(p.x*0.043+p.y*0.032+f[0])
                                    + 0.16*sin(p.x*0.083-p.y*0.068+f[2])
```

Seed the random state from LE64(seed[24..32]). For each byte, add `9e3779b97f4a7c15` hex with 64-bit wrap, set z to the updated state, apply z=(z xor (z>>30))×`bf58476d1ce4e5b9`, then z=(z xor (z>>27))×`94d049bb133111eb`, both with wrap. Return (z xor (z>>31))>>56.

1. Enumerate y then x from 5 through 511 step 6. Add successive random bytes divided by 51.0 to x then y. Retain points inside. Stable-sort ascending by trunc(length(local(p)−anchor)×0.3) + floor(seed[trunc(p.x+p.y) mod 32]/10). Anchor is first core or (crest,−40).
2. Pop from a FIFO of neighbor candidates first, else the next sorted point. Skip unless inside, at least 4.0 from each singularity, and no indexed ridge point is strictly within spacing(p)×0.84. Ridge points are indexed by cell (trunc(x)/12,trunc(y)/12), clamped to 0–42. Search neighboring cells including the central cell, in ascending row and column and insertion order. Choose the strictly smallest squared distance below radius². The earliest matching point wins equal-distance ties.
3. Trace from p along direction(p), adding at most 900 steps. Each step uses d = oriented direction at p+t×(1.8×0.5) with reference t, then next=p+d×1.8. Stop if next is outside, singular within 2.8 or nearer an indexed point than spacing(next)×0.68. If line length >12, next is within 1.8×1.25 of start and dot(d,initial direction)>0.8, append start and return closed. Otherwise stop if next is within 4.0 of any point in the current line in reverse order, skipping eight then stepping by three. Else append next, set p=next and t=oriented direction at p with reference d.
4. Unless closed, trace afresh from the original p in the opposite direction. If the reverse trace is closed use it alone. Otherwise reverse its points, remove the duplicate start at the end, and append the forward trace. Drop lines with fewer than six points.
5. For each segment starting at indices 0,6,12,… of the line before joining, take its start p and normal of the unit segment. Let distance=spacing(p)×(0.97+byte()/2400.0). Enqueue p+normal×distance then p−normal×distance.
6. For an open line, draw a byte and attempt a join at its end if byte < junction rate. Draw another byte and, if it passes the same test, reverse the line and its two endpoint join flags, then attempt a join at the new end. Keep that orientation even if the join fails. A join requires last index ≥10 and tip margin ≥0.09. Set trim=last−4, end=line[trim], incoming=unit(end−line[trim−2]). Find the nearest indexed point to tip+incoming×10 within radius 9. Let chord=target−end, L=length(chord), and outgoing=direction at target oriented by incoming. Require 12≤L≤25, dot(unit(chord),incoming)≥0.82, dot(unit(chord),outgoing)≥0.75, and target outside singularity distance 6. Set a=end+incoming×(L×0.40), b=target−outgoing×(L×0.33), steps=ceil(L/1.8)+2. For i=1..steps, t=i/steps, s=1−t, sample end×(s×s×s)+a×(3×s×s×t)+b×(3×s×t×t)+target×(t×t×t). Every sample must be inside, and samples farther than 5 from target must have no indexed point within 3.5. On success truncate after trim, append samples and mark the endpoint joined.
7. Index the finished line points in order. Draw jitter=(byte()/255.0−0.5)×0.54. For vertex j the start and end distances in indices are j and last−j, respectively, each replaced by 4 for a joined endpoint. The width at vertex j is thickness × taper. For closed lines taper is 1, otherwise it is 0.54+0.46×min(1,min(start,end)/4.0).
8. Rasterize segments in order into an initially zero 512×512 plane. For endpoint widths wa, wb, expand the axis-aligned endpoint bounds by max(wa,wb)+0.8, round floor on left/top and ceil on right/bottom, clip to canvas and iterate inclusively. For pixel center u and segment from a to b, v=b−a, offset=u−a, t=clamp(dot(offset,v)×(1/max(dot(v,v),1e−9)),0,1), distance=length(offset−v×t). Compute alpha=clamp(wa+(wb−wa)×t−distance+0.5,0,1). Store max(old,trunc(alpha×255+0.5)). Repeat until both candidate sources are exhausted. The RGB pixel is (plane value,0,0).

### Shared PNG and fingerprint commitment

Both chain renderers encode an 8-bit RGB, non-interlaced raster in row-major order with `png` 0.18.1, compression preset Fast, default adaptive filter, and no added text or ancillary metadata. For exact bytes use the same encoder behavior, including `fdeflate` 0.3.7, row-filter selection, fallback to stored blocks when its stream would be larger, and IDAT chunk framing. For each row, evaluate PNG filters Up, Sub, Average and Paeth in that order at three bytes per pixel, with a zero previous row initially. Score the residual bytes by summing the absolute value of each byte interpreted as signed i8. Take the first score of zero immediately, otherwise the lowest score with ties resolved in favor of the later filter. Emit its filter byte followed by residuals. Stored fallback uses unfiltered rows. These images use one IDAT chunk between IHDR and IEND. An arbitrary lossless PNG can preserve pixels while producing a different commitment. The release does not supply a format-independent canonical PNG or cross-platform proof of identical floating-point rasterization. A compatible implementation can use the pinned general-purpose encoder and numerical library without the indexer. Compare the golden hash in §13 before relying on another encoder or floating-point implementation.

Fingerprint artwork JSON contains `name` = `<E>.fingerprint`, `type` = `soq-fingerprint`, `content_sha256` = hex SHA-256 of complete PNG bytes, and `parameters` with `renderer`, `creator_script`, `txid`, `vout`, `block_hash`, `height` and lowercase `pattern`. Hash the compact UTF-8 JSON object with SHA-256 to get the asset metadata hash. For all generated assets in this spec, object keys are lexicographically sorted recursively as in the release's serde_json Value maps; use JSON integers, no spaces or final newline, and quoted strings. Runtime decorations such as `asset_origin_txid`, `identity_height`, `source_reorganized` and `prior_block_hashes` are outside the commitment.

## 9. SOQ-MAPS

### On-chain quadtree maps (08)

Require the map fee, witness creator from input 0, funded v1 destination, and for every input prevout at height U require payment height P ≥ U+5. This is a funding rule additional to Core validity. No user-authored tile list, coordinates, palette, name, claimed height, or renderer version is accepted from the carrier. The ID is the request txid.

At the end of tip T ≥ 5, find all unassigned requests with P≤T−5 in block/transaction order. Starting at next=1+maximum assigned height (or 0), assign each E=max(next,P+5), then set next=E+1. Future E values are recorded immediately. Next reveal unrevealed assignments with E≤T−5 in ascending E order using their canonical block hashes. Entropy heights and block hashes are unique within this family, independent of the fingerprint queue. Create one note at the original destination even if spent. Before reveal there is no transferable note. After reveal the usual spending rules apply.

### Quadtree generation

Seed = SHA256(`SOQ.MAPS\0seed\0v1\0` || displayed entropy hash bytes[32] || displayed request txid bytes[32] || raw creator script[34]). Height and vout do not enter this seed. For domain D form K=SHA256(seed || D as one byte || BE64(0)). Each domain has an independent counter starting at 0. To draw below positive N, hash K || BE64(counter), increment the counter on every attempt and read the first eight bytes as BE64 r. Return r mod N on the first r ≥ 2⁶⁴ mod N. Domains 1 through 6 respectively select shape, geometry, rarity, color, colored count and element positions.

```text
bubble = D1.below(10000) == 0
R = D2
scale = 1 << R.below(10)
count = scale + R.below(scale)
if count == 1:
    squares = [(0,0,24 + R.below(521))]
else:
    cells = [(0,0,400 + R.below(145))]
    capacity = count + R.below(1 + floor(count/3))
    while len(cells) < capacity:
        weight = side^3 if side >= 12 else 0, for each cell in order
        draw = R.below(sum(weights))
        remove first cell whose cumulative upper bound exceeds draw
        half = floor(side/2); far = side-half
        append (x,y,half), (x+far,y,half), (x,y+far,half), (x+far,y+far,half)
    for i descending from len(cells)-1 to 1:
        swap cells[i], cells[R.below(i+1)]
    for (x,y,c) in first count cells in shuffled order:
        low = max(3,floor(2*c/3))
        side = low + R.below(c-1-low)
        append (x+1+R.below(c-side-1), y+1+R.below(c-side-1), side)
recenter by subtracting min x and min y and adding
    floor((576-(max(x+side)-min x))/2) and the analogous y offset
```

Count is 1–1023. Splits use ordered removal and append. For odd sides the split leaves a pixel gap. Shrink and offset add an inner margin for each selected tile. Palette in order is `00bfff ff00a8 a855ff 39ff14 ff6700` in RGB hex. Initialize every element blue. Rainbow applies if D3.below(1000)=0 and count≥5. Otherwise call D3.below(100); zero enables an accent. For either case start with position array [0,…,count−1]. For rainbow, perform the first five steps of forward Fisher–Yates, swapping i with i+D6.below(count−i), and set the resulting position at i to palette color i. For remaining positions in that array order, select colors with D4.below(5). For accent, select color at 1+D4.below(4), set k=1+D5.below(count), and run the first k steps of the same shuffle and assign the accent to each selected position.

Rasterize into a black 576×576 RGB canvas in array order and for each square fill dx,dy in [0,side). For bubbles retain the pixels satisfying `abs(2*dx+1-side)^2 + abs(2*dy+1-side)^2 <= side^2`. Encode and hash the PNG as in §8. Artwork fields are `name` = `<E>.soqumap` for both shapes, `type` = `soq-map`, `content_sha256` = PNG digest, and `parameters` with `renderer` = `soq-maps-quadtree-2`, `height` = E, `block_hash`, `shape` = `square` or `bubble`, `txid`, `creator_script` and `geometry_count`. The metadata commitment hashes this compact JSON. The chain has no tile edits or secondary map operations.

### Separate SVG renderer (library only)

`soq-maps-1` accepts a lowercase displayed 32-byte block hash and a height. It has no active opcode or legacy fallback. For each ASCII domain `type`, `color`, `geometry`, its independent stream hashes `SOQ.MAPS\0v1\0` || domain || NUL || displayed hash bytes || LE64(counter), starting at counter zero. Interpret the first four bytes as LE32 r and reject r ≥ 2³²−(2³² mod N) before returning r mod N. Bubble iff type draw below 10000 is zero. Color draw below 100 selects copper, blue, green, purple at 0–3, black otherwise. Foregrounds are respectively `#D96F32`, `#2563EB`, `#10B981`, `#9333EA`, `#000000`.

Target = 8+geometry draw below 57. Attempt at most 4096 proposals, stopping at target. Size=8+draw below 65. For a square choose x and then y as 16+draw below (480−size). For a bubble choose center coordinates as 16+size+draw below (480−2×size). Reject a square if for an existing square both axes overlap under strict inequalities with a 4-pixel added margin at the ends. Reject bubbles where squared center distance < (sum of radii+4)². Preserve accepted order. Generate the UTF-8 SVG as a single line without newline using the following exact components and substituting decimal integers and the foreground.

```text
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"><rect width="512" height="512" fill="#FFFFFF"/><g fill="{foreground}">
<rect x="{x}" y="{y}" width="{size}" height="{size}"/>
<circle cx="{x}" cy="{y}" r="{size}"/>
</g></svg>
```

Use the rect or circle template for each accepted element according to shape. Content hash is SHA-256 of SVG bytes. Name suffix is `.soqmap` for square or `.soqububblemap` for bubble, preceded by decimal height. Metadata fields are `name`, `type` = `soq-map`, `content_sha256`, `geometry_count`, and `parameters` with `block_hash`, `height`, lowercase `shape`, lowercase palette name and `renderer` = `soq-maps-1`. The library recomputes the canonical shape, color and version from inputs and rejects manually changed parameters.

## 10. FATCAT

FATCAT records paid requests and assigns unique atlas variants (0–1005) and display numbers (1–1006). The fixed atlas RGB SHA-256 commitment is `df4769c2823249ae11cec99b559c47fe86ef954876d62f38f36f0b5d4dfad1ee`. The request txid is the asset ID. The carrier has no salt, assigned variant, display number, metadata commit or separate reveal message. The first commitment is the on-chain payment/request. The indexer derives the reveal and its metadata commitment from a later block and its journal. The release does not reconstruct the atlas RGB image from the commitment.

### Historical claim issuance and payment

**Single issue (0c).** Input 0 must spend a prevout with the platform fee script. The store exposes a test issuer override requiring that same witness script shape and immutable once set. Reproducing a test store requires its override. Require a funded v1 destination, but no exact anchor amount for a single issue. Reject a serial present in any accepted request, retained draw, or prior claim whose output is unspent or was spent at height U with U+5>H. An abandoned claim can be reissued once the spend has six confirmations by H if no request or draw reserves that serial. Issuance does not mint an NFT.

**Pool issue (0f).** For count N require N+1 or N+2 outputs, carrier at N with zero atoms, and outputs 0 through N−1 with the same v1 script distinct from the platform fee script, each at 1,000,000 atoms. The optional last output has no additional operation-specific constraints. Apply single issuance for serial first+i at destination i, tag each claim as pooled, and reject the whole operation on any failure.

**Legacy payment (0d or 0e).** Reject if any accepted request or retained draw has a negative internal serial (direct mode has started). Require destination 0, three or four outputs, zero-atom carrier at output 1 and exactly 200,000,000,000 atoms at output 2 to the platform. Input 0 must spend the appropriate single or pooled claim, and H ≥ claim output height + 5. For single claims the owner script is the claim script. For pooled claims require at least one more input, and take owner from output 0, distinct from the claim script. Output 0 must fund that owner at 1,000,000 atoms with the v1 script shape. Optional output 3 must have positive value and the owner script. Each additional input must be an owner-script non-coinbase output at least six confirmations deep at H, and have no claim, FATCAT request, asset note, fingerprint request or SOQ-MAPS request anchored there. Reject an existing draw for this serial unless its request txid is exactly this transaction. Insert a request unique by both txid and serial. Preflight also rejects requests when halted; confirmed legacy payments only check the direct-mode gate and ordinary rules. Claim issuance does not check either gate.

### Direct request (10)

Require the same carrier and 2,000 SOQ fee layout and the same exact output-0 amount. Require v1 creator from input 0 and all input prevouts to meet H ≥ funding height+5 using the recorded payment height H. Output 0 and positive optional change output 3 must use the creator script. The direct path has no additional confirmed-path claim/funding-input exclusion or separate coinbase check. Reject if halted or if any positive-serial accepted request has no draw.

Let D be the set union of every journaled display number and every journaled variant plus one. Let Q count accepted requests without a draw. Confirmed direct availability is max(0,1006−|D|−Q). Reject if zero. Each accepted direct request gets an internal serial one less than the minimum of zero and all serials in requests and journal rows. Negative serials mark direct requests and do not select the art. In preflight, additionally subtract with saturation up to 1006 observed, non-confirmed transactions whose output 1 has the exact zero-valued direct destination-0 script and whose output 2 has the exact fixed payment. This is a reservation heuristic using two outputs, without full validation of those candidates. Earlier transactions in a block reserve capacity first.

### Reveal and draw journal

At the end of block H, if H≥5 and not halted, take undrawn requests with payment height exactly H−5 in their transaction order. Use the displayed hash of block H as entropy. Construct ascending lists from the full journal, including any retained entries. V contains unused variants, N unused numbers, and P the subset of N whose variant number−1 is unused.

```text
draw_index(domain, hash, txid, bound):
    threshold = 2^64 mod bound
    for counter starting from 0:
        bytes = "SOQ.MARKET FATCAT draw v1\0" || domain ||
                displayed_hash_bytes || displayed_txid_bytes || LE64(counter)
        ticket = LE64(SHA256(bytes)[0..8])
        if ticket >= threshold: return ticket mod bound

for request in payment-block transaction order:
    if serial < 0:
        number = remove P[draw_index("paired-number\0", H_hash, txid, len(P))]
        variant = number - 1
    else:
        variant = remove V[draw_index("variant\0", H_hash, txid, len(V))]
        number  = remove N[draw_index("number\0", H_hash, txid, len(N))]
    construct metadata, hash its bytes, record draw, and create one NFT note
```

The legacy draws remove independently from V and N, and direct draws remove only from P. The request gates prevent an undrawn mixed-mode cohort under normal operation. Empty required candidate lists, uniqueness failures or any other finalizer failure abort block application rather than rejecting just the payment. Each journal entry stores txid, serial, variant, number, reveal height/hash, metadata digest and bytes. The journal enforces unique txid, serial, variant, number and metadata hash. It is also the source for status and artwork metadata after rollback.

Use the compact sorted-key JSON convention in §8. Fields are `name` = `FATCAT#<number>`, `symbol` = `FATCAT`, `fatcat_variant`, `fatcat_number`, `atlas_rgb_sha256` = fixed hex above, `reveal_height`, `reveal_block` = displayed block hash, `mint_txid` = displayed request txid, `draw_version` = 1 for legacy or 2 for direct, and `claim_serial` = positive serial for legacy only. Asset metadata is SHA-256 of these JSON bytes. Credit the original output 0 at reveal even if spent, in which case it has no live balance. The asset can subsequently be transferred, settled or cancelled under SOQ-721 note rules.

## 11. Confirmations and mempool

An indexed transaction at H has T−H+1 confirmations at indexed tip T. Zero means the transaction is absent from the indexed chain, even if independently observed in the mempool. Deploys, ordinary mints, collection and NFT operations, receipts and transfers take effect on inclusion. The funding-confirmation helper returns one for Transfer, Settle, Cancel and Listing and six for other operations. This helper is not a blanket confirmed-validator rule. Explicit input-age checks apply to SOQ-MAPS and FATCAT as documented. Fingerprints and SOQ-MAPS wait for both payment and assigned entropy maturity, earliest reveal P+10. FATCAT reveals at P+5.

The CLI in follow mode refreshes the observable mempool at the current tip, polling Core IDs and fetching missing bodies. It stores observed IDs, inputs and outputs, retires disappeared entries, and removes entries on confirmation. It derives protected destinations by decoding carriers even when a confirmed validator might reject them. Mempool observations can alter projected spendable coin funding, pending status, listing visibility, discount eligibility in preflight and direct FATCAT availability. They never credit confirmed tokens or NFT ownership. Canonical processing ignores mempool policy and ordering.

Name preflight checks existing names/tickers, then observed single-carrier creation candidates that parse, have a syntactically valid claim, and have exactly one platform output at a full or discount-eligible value. That reservation check does not require carrier adjacency or full state validation. It also checks decodable locally prepared or broadcast creations. The provisional reservation does not decide a confirmed conflict. Preflight simulates the confirmed asset operation in a rollback-only transaction, with historical-fee mode disabled. It frames operation eligibility at T+1 but inserts simulated transaction and output rows at T; generated funding helpers that read the transaction record therefore use T. It requires canonical unspent funding, protects noted outputs and artwork/claim anchors from unintended spending, and applies the local listing rules. In particular, confirmed spends of protected anchors are still possible. The standalone CLI does not broadcast.

## 12. Rollback and replay

Core chooses the active branch. The indexer checks its tip against the node's hash at the same height, rolling back one block per mismatch or when its tip is above the node. It errors on a genesis mismatch. For replacement blocks it fetches ordered contiguous windows and rechecks the last hash before applying. Transaction ordering within replacement blocks determines fresh conflict winners.

For rollback beginning at R, the first height removed, perform atomically:

1. **FATCAT:** if any journal entry has reveal height ≥ R, persist the `fatcat_halted` flag with value `reveal_reorganized`. Delete the derived NFT assets at those reveal heights and their notes, even if their payments survive. Keep every draw and its bytes, including the number and variant reservations. No replacement draw is produced even if the old entropy block later becomes active again. The flag survives restart. All FATCAT finalization and direct admissions stop. Legacy confirmed admissions can still pass their ordinary gates, with no draw while halted. Claim issuance, already-live assets from unaffected reveals, base chain sync and other protocols continue. There is no automatic unhalt or administrative reset exposed by this CLI.
2. **Generated art:** remove fingerprint and SOQ-MAPS reveals and their assets where entropy height E+5 ≥ R. Remove SOQ-MAPS assignments whose payment height P+5 ≥ R. Keep surviving mature assignments even if their entropy block changed or is temporarily unavailable. Unaffected requests remain queued. A shared reveal audit retains each distinct (request txid, entropy block hash) and associated height. On a later reveal, query decorations report prior hashes different from the current hash; these records do not alter queue ordering, art or ownership.
3. **Base and application state:** clear observed mempool, mark tracked submissions confirmed in the removed range as reorganized, and delete transactions, outputs and blocks at heights ≥ R. Remove transaction-dependent notes, mints, names, collections, memberships, requests, claims, receipts, errors and trades. Local listings attached to deleted outputs disappear. A surviving output spent by a removed transaction becomes unspent, restoring any still-existing note and its balance. Preserve separately tracked raw submissions and independent journals.
4. Resume forward application from the surviving checkpoint. Fetch a fresh mempool observation when following the node at its current tip.

A clean canonical replay can reproduce reversible asset state under the same profile and renderer behavior. FATCAT after a halt requires the same observed branch history or equivalent retained journal and halt state. Canonical chain data alone cannot recover removed draws or determine whether a database ever halted. Fingerprint and map prior-reveal decorations and market views also depend on local observations. A clean replay and a migrated database can differ as described in §5. Archive state needed for the intended replay mode.

## 13. Verification vectors

All examples in this section are synthetic. The ledger fixtures model decoded blocks and may spend coinbases too early for Core consensus. Use them to verify application interpretation. Below, `H(n)` is n formatted as 64 lowercase hex digits, `S(b)` is `5120` followed by 32 copies of byte b, and `[xx]×n` means n repetitions of that hex byte. Spaces and bars in hex layouts are presentation only. An exact script is only an application carrier; its transaction must separately meet the layout, fee and prevout rules.

### Carrier and accounting vectors

| Case | Input or exact script template | Expected result and basis |
| --- | --- | --- |
| Named deploy | `6a 4c 4e 53514d4b0102 [01]×32 0a00000000000000 0600000000000000 5445535400000000 7b274e01b7610268ddcb0bd5b1c3f2b9` | Cap 10, limit 6, ticker TEST, Test Token key. Fixture deployment txid H(10), zero minted. [Ledger tests, fees/caps][ledger] |
| Mint | `6a 2f 53514d4b0103 H(10) 0600000000000000 00` | Fixture tx H(11) credits 6 at S(01). Next mint of 5 fails cap. Mint of 4 succeeds and reaches cap 10. [Ledger tests, fees/caps][ledger] |
| Transfer | `6a 30 53514d4b0104 H(10) 01 00 0600000000000000` | Derived from the six-unit fixture lot. Spending H(11):0 with a 1 SOQ fee after the carrier and an exact token anchor to S(02) at output 0 gives S(02) six units and leaves four with S(01) if applied before the settlement. Changing the allocation to five or adding a second OP_RETURN rejects credit while the canonical spend still consumes the lot. [Transfer validator][assets] |
| Settle | `6a 36 53514d4b010a H(10) 0600000000000000 00c2eb0b00000000` | Fixture H(14), price 200,000,000 atoms. A=1,000,000, each fee=1,000,000, payout=200,000,000, fee output=2,000,000. Buyer gets 6, seller retains 4. Rollback from height 6 restores seller's 10, removes trade and restores listing visibility if locally registered. [Ledger tests, fees/caps][ledger] |
| Collection member | `6a 47 53514d4b0107 H(1) [09]×32 00` | H(1) has maximum 1 and creator S(01). H(2) with S(02) creator fails; H(3) without fee fails; H(4) with S(01) and fee creates one note; H(5) exceeds maximum. [Ledger tests, collection mints][ledger] |
| NFT transfer | `6a 30 53514d4b0104 H(106) 01 00 0100000000000000` | Fixture tx H(110) after FATCAT reveal spends H(106):0 and pays 1 SOQ. Destination S(03) owns NFT, S(02) loses ownership. [Ledger tests, FATCAT claim/payment][ledger] |
| Duplicate creation | Same folded key from Straße and STRASSE, different token tickers, or duplicate TEST ticker with another key | First valid token wins. Conflicting token operations fail. A collection namespace is independent. Removing the winning block releases the reservations. [Ledger tests, creation names][ledger] |
| Obsolete precision | Start with a 62-byte Deploy payload, insert 00 at payload offset 54 and change the direct push length to 63 | Reject even with zero precision. Nonminimal PUSHDATA1 for a 7-byte request also rejects. [Carrier tests][carrier] |

### Fingerprint and map vectors

- **Fingerprint golden.** Creator S(01), txid `ab` × 32, vout 3, entropy hash `cd` × 32, E=20. Seed `72491bae4be33f18c7881ad9bb3f36dfb512b3dc34775b795308d6cd71d1d6ee`; pattern arch. Encoded PNG SHA-256 `6820f25b5f358f08284e8fbe408a1fdad6208344933d82cdc4c6490c3e90b67e`. Changing vout or creator changes the PNG. [Fingerprint golden test][fingerprint]
- **Fingerprint ordering and rollback.** Script `6a0753514d4b010500`. Requests H(900) then H(100) at height 1 reveal first at tip 11 (E=6) and then at tip 12 (E=7). The first request's seed uses creator S(01) and its note belongs to S(02). An unpaid third request H(99) produces no note. Rollback at 6 removes both reveals. With replacement hashes H(200000+h), H(900) reveals again at E=6 using H(200006), with a changed content digest and prior hash H(100006) in the audit view. [Ledger tests, fingerprint replay][ledger]
- **Quadtree request and reassignment.** Script `6a0753514d4b010800`. At block 7, accept H(900) then H(100), funded at heights 1 and 2. Reject H(800) funded at 6 and unpaid H(700). At tip 12 assign 12 and 13 respectively, neither revealed. At tips 17 and 18 reveal respectively. Rollback at 7 and reapply them in reverse order at height 7. At tip 18, H(100) has `12.soqumap`, H(900) has `13.soqumap`, and H(900)'s prior reveal hash is available for audit. Old content is removed from canonical retrieval. [Ledger tests, MAPS FIFO][ledger]
- **Quadtree raster.** For creator S(07) and txid `12` × 32, scan H(n) for n=0..99,999 until the domain-1 ticket is zero. At E=123 the first hit is H(24). The renderer produces `123.soqumap` with three bubbles, black origin and the selected color at each tile center. The release encoder emits 8,835 bytes with SHA-256 `2cfc9f5c29ceff42780dd0f98eed602aad8fc5b7558fff6a1300a251f3026f96`; metadata SHA-256 is `c7ddd93520aea088264aa06aa9fe6c6875dea07e7ea0cf73b9dad93015a4186c`. The exact digests are additional outputs calculated from the renderer test inputs. [Quadtree renderer test][quadtree]
- **SVG library golden.** For hashes consisting of 32 repetitions of byte n, with height n, use n=0,1,42,127,254,255 in that order. The expected six SVG SHA-256 outputs are the six lines in the fixture. For n=0 the output is `bd6469dbf340c2a7834c1aa27c5668f3d7d35f25285919481168fe0953159c54`. These are library vectors. [Golden test][maptest] [Expected digests][mapfixture]

### FATCAT vectors

- **Claim and legacy draw.** Claim serial 1 to output 0 uses `6a0953514d4b010c010000`. In the historical ledger fixture, the test issuer S(01) issues H(101) to S(02) at height 2. The second issuance H(102) for serial 1 fails. H(106) pays at height 8 using `6a0753514d4b010d00` with the exact platform fee, while H(107) pays one quarter and fails. At tip 13 using entropy H(100013), the counter-zero tickets for variant and number are respectively 7777215406811656881 and 460970662973286437. With empty lists of prior draws, these yield variant 521 and number 408. SHA-256 of the metadata bytes is `d549df85f569ef90e4893887e4f6fe19cfde890795427617edce378d2af8deb3`. Exact tickets and digest are calculated from fixture inputs using §10. Rollback from 13 keeps the draw, removes its asset and halts FATCAT even though the payment remains confirmed. [Ledger tests, FATCAT claim/payment][ledger]
- **Direct draw and capacity.** Script `6a0753514d4b011000`. With no draws before the cohort at height 6, its first request H(20000) uses entropy H(100011); counter-zero ticket is 4148757212369711169, assigning number 364 and variant 363. Metadata SHA-256 is `5af5b51dc613172574aaee7e2cef53dfebd895b48753707a19861c0211e4d969`. The fixture fills 1005 slots at payment height 6 and one at height 7, rejecting the later same-block competitor. Draws reach 1005 at tip 11 and 1006 at tip 12, with all numbers used and variant=number−1. Rollback from 12 retains all 1006 journal records and sets halt. The ticket and digest here are calculated from the fixture inputs. [Ledger tests, direct FATCAT][ledger]

## 14. Limits and independent implementation

The chain reader accepts up to 100,000 transactions per block and caps an RPC response at 64 MiB. These are implementation limits. The separate raw transaction decoder used by library submission workflows limits raw data and each length-prefixed blob to 2,000,000 bytes, and input, output and per-input witness item counts to 100,000 each. Neither its limits nor wallet selection limits define Core consensus or a bound on parsed block transaction inputs. The application carrier is bounded at 80 bytes, transfer fan-out at four, and a pool issue at 64 outputs. Amount conversions, supply increments, payout and commission arithmetic, and allocation totals have the bounds described above. Invalid payload processing still requires indexing its base transaction. Admission cost also includes indexed queries, and bursts of mature requests can require multiple deterministic renders in one block. Neither the platform fee nor bounded scripts guarantees bounded total indexing work.

Duplicate transactions and duplicate outpoints violate base-index uniqueness. Logical duplicates use operation rules: names and tickers are first-wins, collection capacity is consumed by accepted members, and FATCAT serial and draw history are reserved as above. Receipts and repeated mints with distinct txids use their own validations. Application replay prevention consists of chain anchoring, outpoint consumption and these namespace rules. There is no separate signed chain ID or application nonce. Creator authorization, transfer authorization and token destruction depend on which UTXOs Core establishes as spent. Name keys are truncated commitments and cannot establish ownership of a name preimage. Block-dependent art and draws inherit the predictability and potential manipulation of their entropy source. The release establishes no independent randomness or fairness guarantee.

To implement independently:

1. Bind to the agreed genesis and a validated Core active chain. Start from genesis or from a complete, branch-identified checkpoint of equivalent state. Read raw locking scripts and exact values, and preserve the two array orders.
2. Keep base prevouts and spend markers. Keep asset definitions, mint totals, collection membership and authority, names and tickers, note history, paid requests and assignments, claims, draws and reversible per-block effects. Use historical notes even if their outputs are already spent, where a transition requires them.
3. Decode one carrier per candidate and apply fees, historical gates, creation and ownership rules against the current ordered state. Record rejection separately from base spends.
4. Run end-of-block finalizers exactly once in order. Persist assignments, journal rows, generated-art metadata and the checkpoint atomically. Verify ledger state (IDs, ownership, caps, heights), then the render and serialization hashes.
5. On branch change, reverse each removed block and reapply replacement blocks. Preserve FATCAT journal and halt data and optionally preserve prior art audit for equivalent public views. Separate unconfirmed reservations and local offer data from chain state.
6. Test each codec and fee layout, then entire synthetic sequences, invalid spends and replays. Compare both clean and incremental replay, including reorganizations that remove only a reveal while leaving its payment intact.

The release contains no canonical off-chain NFT metadata schema or signed marketplace offer format used by confirmed validation. Reproducing exact generated commitment bytes requires compatible dependency behavior and the same historical renderer profile. The available fixtures verify application rules and a set of render outputs. They do not establish agreement with an independently mined chain, reproduce historical FATCAT forks from current chain data, or certify encoder and floating-point results on every platform.

## 15. Source and test references

| Source | Entry points and evidence |
| --- | --- |
| [Carrier and name helper][carrier] | `Operation::script`, `Operation::parse`, `operation`, `name_key`; carrier round trips and obsolete precision rejection |
| [Platform fees][fees] and [core primitives][core] | Exact and carrier-adjacent payments, discounts, commission, raw transaction encoding, hash order, address and atom helpers; fee and primitive unit tests |
| [Asset ledger][assets] | `apply`, `full_lot`, creation, discounts, notes, finalizers and generated-art rollback |
| [FATCAT][fatcat] | Claim issuance, pool, direct/legacy admission, availability, draw sampling, metadata, status and rollback |
| [Store and synchronization][store] | `open`, integer upgrade, `apply_block`, `rollback`, `next_window`, preflight, mempool, listings, regression tests for migration, snapshots and rollback |
| [RPC][rpc] and [CLI][cli] | Chain and mempool data ingestion, response bounds, txindex probe, checkpoint orchestration, RPC unit tests |
| [Fingerprint seed and goldens][fingerprint], [flow compositor][flow] | Input and output byte conventions, renderer and golden PNG |
| [Quadtree maps][quadtree], [SVG maps][svg] | Active and library-only rendering, deterministic geometry and raster tests |
| [Ledger integration tests][ledger], [CLI tests][clitest], [SVG golden test][maptest], [SVG digests][mapfixture] | Synthetic ledger outcomes, argument restrictions, exact SVG verification |
| [Workspace dependencies][manifest], [dependency lock][lock], [release README][readme] | Versions, serialization features, operating assumptions and database behavior |

[carrier]: crates/soq-core/src/protocol.rs
[fees]: crates/soq-core/src/fees.rs
[core]: crates/soq-core/src/lib.rs
[assets]: crates/soq-indexer/src/assets.rs
[fatcat]: crates/soq-indexer/src/fatcat.rs
[store]: crates/soq-indexer/src/lib.rs
[rpc]: crates/soq-indexer/src/rpc.rs
[cli]: crates/soq-indexer/src/bin/soq-indexer.rs
[fingerprint]: crates/soq-fingerprints/src/lib.rs
[flow]: crates/soq-fingerprints/src/flow.rs
[quadtree]: crates/soq-maps/src/synthwave.rs
[svg]: crates/soq-maps/src/lib.rs
[ledger]: crates/soq-indexer/tests/ledger.rs
[clitest]: crates/soq-indexer/tests/cli.rs
[maptest]: crates/soq-maps/tests/golden.rs
[mapfixture]: tests/fixtures/maps-v1-golden.txt
[manifest]: Cargo.toml
[lock]: Cargo.lock
[readme]: README.md
