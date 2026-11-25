// Extracted from SOQ.MARKET web/app.js. The host supplies wallet state and controls.
// Initialize the actual generated wasm-bindgen module with the matching network
// configuration before passing its BrowserWallet instance into this adapter.
export function walletBoundary(state, walletControls) {
async function api(path,options){const response=await fetch(path,options);let data;try{data=await response.json();}catch{throw new Error(`Server returned ${response.status}`);}if(!response.ok)throw Object.assign(new Error(data.error||`Request failed (${response.status})`),{status:response.status});return data;}
function post(path,body,keepalive=false){
 const payload=JSON.stringify(body);
 return api(path,{method:'POST',headers:{'content-type':'application/json'},body:payload,
  keepalive:keepalive&&payload.length<60_000});
}
function coins(value){let amount=BigInt(value);const whole=amount/100000000n;const fraction=(amount%100000000n).toString().padStart(8,'0').replace(/0+$/,'');return whole.toString()+(fraction?'.'+fraction:'');}
function wallet(){if(!state.wallet)throw new Error('Browser cryptography unavailable');state.wallet.address();return state.wallet;}
const walletStorageKey='soq-encrypted-wallet';
function storedWallet(){return localStorage.getItem(walletStorageKey);}
function persistNewWallet(password){try{const encrypted=state.wallet.encrypted_backup(password);if(storedWallet())throw new Error('An encrypted wallet already exists.');localStorage.setItem(walletStorageKey,encrypted);if(storedWallet()!==encrypted)throw new Error('Unable to save encrypted wallet.');}catch(error){state.wallet.lock();walletControls(false);throw error;}}
async function networkFeeQuote(raw){
 const quote=await post('/api/v1/fee-estimate',{raw});
 const fee=BigInt(quote.network_fee_atoms);
 if(fee<=0n)throw new Error('The node cannot estimate a network fee. Please try again later.');
 return {quote,fee};
}
async function estimate(build,recordQuote=true,acceptNoChange=false){
 let fee=1n;
 for(let i=0;i<12;i++){
  const raw=build(coins(fee));
  const {quote,fee:next}=await networkFeeQuote(wallet().fee_template(raw));
  // A fee that consumes the remainder can remove the change output and reduce the quote.
  if(next===fee||(acceptNoChange&&next<fee&&JSON.parse(raw).change_atoms==='0')){
   if(recordQuote)state.lastQuote=quote;
   return raw;
  }
  fee=next;
 }
 throw new Error('The network fee did not stabilize. No transaction was signed. Refresh and try again.');
}

// These calls are extracted from broadcastReview. The host invokes this
// boundary after the authorization handler, with the same review string.
async function submitAuthorized(raw) {
 const parsed=JSON.parse(raw);
 const signed=wallet().sign_review(raw);
 const id=wallet().transaction_id(signed);
 return post('/api/v1/broadcast',{raw:signed,expected_txid:id,network_fee_atoms:parsed.network_fee_atoms,authorized:true},true);
}

function unlockStoredWallet(password) {
 const encrypted=storedWallet();
 if(!encrypted)throw new Error('No encrypted wallet is stored.');
 return state.wallet.unlock_backup(encrypted,password);
}

return {storedWallet, persistNewWallet, unlockStoredWallet, estimate, submitAuthorized};
}
