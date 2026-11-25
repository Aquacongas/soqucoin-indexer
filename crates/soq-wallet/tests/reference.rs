use fips204::{
    ml_dsa_44::{PublicKey, KG},
    traits::{KeyGen, SerDes, Verifier},
};
#[test]
fn official_soqucoin_keygen_and_signature() {
    let fixture = include_str!("../../../tests/fixtures/official-mldsa44.txt");
    let mut lines = fixture.lines();
    let pk: [u8; 1312] = hex::decode(lines.next().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let sig: [u8; 2420] = hex::decode(lines.next().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let (ours, _) = KG::keygen_from_seed(&[7; 32]);
    assert_eq!(ours.into_bytes(), pk);
    let key = PublicKey::try_from_bytes(pk).unwrap();
    assert!(key.verify(&[42; 32], &sig, &[]));
    assert!(!key.verify(&[43; 32], &sig, &[]));
}

#[test]
fn encrypted_backup_roundtrip_and_tampering() {
    let wallet = soq_wallet::Wallet::from_seed([7; 32]).unwrap();
    let password = "a sufficiently long local password";
    let encrypted = wallet.encrypt(password, "sq", 158).unwrap();
    let recovered = soq_wallet::Wallet::decrypt(&encrypted, password, "sq", 158).unwrap();
    assert_eq!(
        recovered.receive_address("sq").unwrap(),
        wallet.receive_address("sq").unwrap()
    );
    assert!(
        soq_wallet::Wallet::decrypt(&encrypted, "incorrect but long password", "sq", 158).is_err()
    );
    assert!(soq_wallet::Wallet::decrypt(&encrypted, password, "ssq", 253).is_err());
    let mut value: serde_json::Value = serde_json::from_str(&encrypted).unwrap();
    value["ciphertext"] = serde_json::json!("00".repeat(48));
    assert!(soq_wallet::Wallet::decrypt(&value.to_string(), password, "sq", 158).is_err());
}
