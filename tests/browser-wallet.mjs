// Development-only WASM conformance test; no RPC, network requests or broadcasts.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { webcrypto } from 'node:crypto';
import init, { BrowserWallet } from '../target/wallet-web/soq_wallet.js';
if (!globalThis.crypto) globalThis.crypto = webcrypto;
await init({module_or_path:await readFile(new URL('../target/wallet-web/soq_wallet_bg.wasm',import.meta.url))});
const recovered = new BrowserWallet('sq',158);
const phrase = 'abandon '.repeat(23) + 'art';
assert.equal(recovered.import_recovery_phrase(phrase), 'sq1p90fh0mklmtkf9ae9vpurs0rj203hzeavhwj0eluutxl647kffczs0evyz3');
assert.equal(recovered.backup_recovery_phrase(), phrase);
assert.equal(recovered.backup(), 'QTqtZjZzSguz2R7fgk9QHiMa4WYKi6j6Cko6C4oCbJ8SMbTzW1c4');
const recoveryEnvelope = recovered.encrypted_backup('correct horse secret password');
assert.equal(JSON.parse(recoveryEnvelope).version, 2);
recovered.lock();
assert.equal(recovered.unlock_backup(recoveryEnvelope,'correct horse secret password'), 'sq1p90fh0mklmtkf9ae9vpurs0rj203hzeavhwj0eluutxl647kffczs0evyz3');
recovered.free();
const wallet=new BrowserWallet('sq',158);
const address=wallet.create();assert.match(address,/^sq1p/);
const wif=wallet.backup();const second=new BrowserWallet('sq',158);assert.equal(second.import(wif),address);
const encrypted=wallet.encrypted_backup('correct horse secret password');assert.ok(!encrypted.includes(wif));wallet.lock();assert.throws(()=>wallet.address());assert.throws(()=>wallet.unlock_backup(encrypted,'wrong password enough length'));assert.equal(wallet.unlock_backup(encrypted,'correct horse secret password'),address);
const script=Array.from(Buffer.from(wallet.destination_script(address),'hex'));
const utxos=JSON.stringify([{txid:'ab'.repeat(32),vout:0,value:'5000000000000',script:Buffer.from(script).toString('hex'),coinbase:false,protected:false,confirmations:6}]);
const op=wallet.operation_json(JSON.stringify({kind:'map',metadata:'',reference:'',maximum:'',limit:'',ticker:''}));
const rawReview=wallet.prepare_operation(utxos,op,'0.01');const review=JSON.parse(rawReview);assert.equal(review.platform_fee_atoms,'750000000000');assert.equal(review.network_fee_atoms,'1000000');const signed=wallet.sign_review(rawReview);assert.match(wallet.transaction_id(signed),/^[a-f0-9]{64}$/);assert.ok(signed.startsWith('020000000001'));
const mintOp=wallet.operation_json(JSON.stringify({kind:'mint',metadata:'',reference:'12'.repeat(32),maximum:'5',limit:'',ticker:''}));assert.equal(JSON.parse(wallet.prepare_operation(utxos,mintOp,'0.01')).platform_fee_atoms,'100000000');
assert.equal(JSON.parse(wallet.prepare_payment(utxos,second.address(),'1','0.01')).platform_fee_atoms,'0');
for(const [kind,expected] of [['nft','500000000000'],['deploy','1000000000000'],['collection','100000000000']]){const op=wallet.operation_json(JSON.stringify({kind,metadata:'12'.repeat(32),reference:'',maximum:'100',limit:'10',ticker:'TEST',name:'Test Asset'}));const r=wallet.prepare_operation(utxos,op,'0.01');assert.equal(JSON.parse(r).platform_fee_atoms,expected);assert.match(wallet.transaction_id(wallet.sign_review(r)),/^[a-f0-9]{64}$/);}
const protectedNotes=JSON.stringify([{txid:'cd'.repeat(32),vout:0,value:'200000000',script:Buffer.from(script).toString('hex'),coinbase:false,protected:true,asset:'ef'.repeat(32),quantity:'1'}]);assert.throws(()=>wallet.prepare_payment(protectedNotes,address,'1','0.01'));
const offer=wallet.create_offer(JSON.stringify(JSON.parse(protectedNotes)[0]),'10',false,0);const buyer=new BrowserWallet('sq',158);buyer.create();const buyerScript=buyer.destination_script(buyer.address());const buyerUtxos=JSON.stringify([{txid:'34'.repeat(32),vout:0,value:'5000000000',script:buyerScript,coinbase:false,protected:false,confirmations:6}]);const purchase=buyer.prepare_purchase(offer,buyerUtxos,'0.01',0);assert.match(buyer.transaction_id(buyer.sign_review(purchase)),/^[a-f0-9]{64}$/);
assert.equal(JSON.parse(purchase).platform_fee_atoms,'100000000');assert.equal(JSON.parse(purchase).network_fee_atoms,'1000000');
assert.equal(review.transaction.outputs[0].value,1_000_000);
for(const inputValue of ['100000000','1000000']){
 const note={...JSON.parse(protectedNotes)[0],value:inputValue,quantity:'8'};
 const transfer=wallet.prepare_transfer(JSON.stringify([note,...JSON.parse(utxos)]),note.asset,'3',second.address(),'0.01');
 const transferReview=JSON.parse(transfer);
 assert.deepEqual(transferReview.transaction.outputs.slice(0,2).map(output=>output.value),[1_000_000,1_000_000]);
 assert.equal(transferReview.platform_fee_atoms,'100000000');
 assert.match(wallet.transaction_id(wallet.sign_review(transfer)),/^[a-f0-9]{64}$/);
 const offer=wallet.create_offer(JSON.stringify(note),'10',false,0);
 const purchase=buyer.prepare_purchase(offer,buyerUtxos,'0.01',0);
 const purchaseReview=JSON.parse(purchase);
 assert.deepEqual(purchaseReview.transaction.outputs.slice(0,2).map(output=>output.value),[1_000_000_000+Number(inputValue),1_000_000]);
 assert.equal(purchaseReview.platform_fee_atoms,'100000000');
 assert.match(buyer.transaction_id(buyer.sign_review(purchase)),/^[a-f0-9]{64}$/);
}
for(const w of [wallet,second,buyer]){w.lock();w.free();}
console.log('WASM wallet: creation, recovery, encrypted storage, operation fees, signing, asset protection and purchase signing passed. No broadcasts.');
const sizingWallet=new BrowserWallet('sq',158);const sizingAddress=sizingWallet.create();const sizingScript=sizingWallet.destination_script(sizingAddress);
const sizingCoins=[{txid:'56'.repeat(32),vout:0,value:'1000000000',script:sizingScript,coinbase:false,protected:false,confirmations:6},{txid:'78'.repeat(32),vout:0,value:'1000000000',script:sizingScript,coinbase:false,protected:false,confirmations:6}];
const one=sizingWallet.prepare_payment(JSON.stringify(sizingCoins),sizingAddress,'1','0.01');const two=sizingWallet.prepare_payment(JSON.stringify(sizingCoins),sizingAddress,'12','0.01');
assert.ok(sizingWallet.fee_template(two).length>sizingWallet.fee_template(one).length,'Coin selection changes the fee structure');
for(const raw of [one,two]){assert.equal(sizingWallet.fee_template(raw).length,sizingWallet.sign_review(raw).length);const review=JSON.parse(raw);assert.equal(BigInt(review.input_total_atoms)-BigInt(review.change_atoms),BigInt(review.amount_atoms)+BigInt(review.platform_fee_atoms)+BigInt(review.network_fee_atoms));}
sizingWallet.lock();assert.throws(()=>sizingWallet.fee_template(one));sizingWallet.free();
console.log('WASM fee sizing: exact signature lengths, multiple funding inputs, debit/change totals and locked-wallet protection passed.');
