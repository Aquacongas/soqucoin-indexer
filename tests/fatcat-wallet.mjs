// Real browser wallet WASM, offline synthetic confirmed funding UTXOs.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {webcrypto} from 'node:crypto';
import init,{BrowserWallet} from '../target/wallet-web/soq_wallet.js';

if(!globalThis.crypto)globalThis.crypto=webcrypto;
await init({module_or_path:await readFile(new URL('../target/wallet-web/soq_wallet_bg.wasm',import.meta.url))});
const wallet=new BrowserWallet('sq',239);
const address=wallet.create();
const script=wallet.destination_script(address);
const funding={txid:'03'.repeat(32),vout:0,script,value:'250000000000',protected:false,coinbase:false,confirmations:6};
const immature={...funding,txid:'01'.repeat(32),confirmations:5};
const protectedOutput={...funding,txid:'02'.repeat(32),protected:true};
const op=wallet.operation_json(JSON.stringify({kind:'fatcat',metadata:'',reference:'',maximum:'',limit:'',ticker:''}));
const raw=wallet.prepare_operation(JSON.stringify([immature,protectedOutput,funding]),op,'0.05',75);
const review=JSON.parse(raw);
assert.deepEqual(review.transaction.inputs.map(input=>input.previous.txid),[funding.txid]);
assert.equal(review.platform_fee_atoms,'200000000000');
assert.equal(review.discount_percent,0);
assert.deepEqual(review.operation,{FatcatRequest:{destination:0}});
assert.equal(review.transaction.outputs[0].value,1000000);
assert.equal(review.transaction.outputs[2].value,200000000000);
assert.equal(review.transaction.outputs[3].value,49994000000);
assert.match(wallet.transaction_id(wallet.sign_review(raw)),/^[a-f0-9]{64}$/);
assert.throws(()=>wallet.prepare_operation(JSON.stringify([immature,protectedOutput]),op,'0.05',0),/insufficient/);
wallet.free();
console.log('FATCAT WASM signs a direct mint funded by confirmed buyer UTXOs at the exact 2,000 SOQ platform price.');
