use std::{
    fs,
    io::{Cursor, Read},
    sync::{Arc, Barrier},
    thread,
};

use notes_blob::{BlobHash, BlobStore, BlobStoreError, InstallOutcome, ParseBlobHashError};
use tempfile::TempDir;

const CONTENT: &[u8] = b"content-addressed notes attachment";

fn store() -> (TempDir, BlobStore) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = BlobStore::new(directory.path().join("store"));
    (directory, store)
}

fn incoming_files(store: &BlobStore, hash: BlobHash) -> usize {
    let shard = store
        .path_for(hash)
        .parent()
        .expect("shard path")
        .to_path_buf();
    fs::read_dir(shard)
        .expect("read shard")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".incoming-")
        })
        .count()
}

#[test]
fn hash_is_canonical_and_serde_uses_the_text_form() {
    let hash = BlobHash::digest(b"hello");
    let encoded = hash.to_string();
    assert_eq!(encoded.len(), 64);
    assert_eq!(encoded, encoded.to_lowercase());
    assert_eq!(encoded.parse::<BlobHash>(), Ok(hash));

    let json = serde_json::to_string(&hash).expect("serialize hash");
    assert_eq!(json, format!("\"{encoded}\""));
    assert_eq!(
        serde_json::from_str::<BlobHash>(&json).expect("deserialize hash"),
        hash
    );
}

#[test]
fn hash_rejects_noncanonical_text() {
    assert_eq!(
        "00".parse::<BlobHash>(),
        Err(ParseBlobHashError::WrongLength { actual: 2 })
    );
    assert_eq!(
        "A000000000000000000000000000000000000000000000000000000000000000".parse::<BlobHash>(),
        Err(ParseBlobHashError::Uppercase)
    );
    assert_eq!(
        "g000000000000000000000000000000000000000000000000000000000000000".parse::<BlobHash>(),
        Err(ParseBlobHashError::NonHex { index: 0 })
    );
    assert_eq!(
        "G000000000000000000000000000000000000000000000000000000000000000".parse::<BlobHash>(),
        Err(ParseBlobHashError::NonHex { index: 0 })
    );
}

#[test]
fn canonical_path_is_sharded_by_the_first_two_hex_digits() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let store = BlobStore::new(directory.path());
    let hash = BlobHash::digest(CONTENT);
    let encoded = hash.to_string();
    assert_eq!(
        store.path_for(hash),
        directory
            .path()
            .join("blobs")
            .join(&encoded[..2])
            .join(encoded)
    );
}

#[test]
fn installs_opens_and_verifies_a_blob() {
    let (_directory, store) = store();
    let hash = BlobHash::digest(CONTENT);
    let installed = store
        .install_reader(Cursor::new(CONTENT), hash, CONTENT.len() as u64)
        .expect("install blob");
    assert_eq!(installed.outcome, InstallOutcome::Installed);
    assert_eq!(installed.blob.size, CONTENT.len() as u64);

    let mut bytes = Vec::new();
    store
        .open_verified(hash, CONTENT.len() as u64)
        .expect("open verified blob")
        .into_file()
        .read_to_end(&mut bytes)
        .expect("read blob");
    assert_eq!(bytes, CONTENT);
    assert_eq!(store.verify(hash).expect("verify blob"), installed.blob);
    assert_eq!(incoming_files(&store, hash), 0);
}

#[test]
fn verified_open_is_bounded_and_rewinds_the_hashed_handle() {
    let (_directory, store) = store();
    let hash = BlobHash::digest(CONTENT);
    store
        .install_reader(Cursor::new(CONTENT), hash, CONTENT.len() as u64)
        .expect("install blob");

    assert!(matches!(
        store.open_verified(hash, 4),
        Err(BlobStoreError::TooLarge { limit: 4 })
    ));
    let mut bytes = Vec::new();
    store
        .open_verified(hash, CONTENT.len() as u64)
        .expect("open verified blob")
        .into_file()
        .read_to_end(&mut bytes)
        .expect("stream verified bytes");
    assert_eq!(bytes, CONTENT);
}

#[test]
fn installs_from_a_file_without_loading_it_whole() {
    let (directory, store) = store();
    let source = directory.path().join("source.bin");
    fs::write(&source, CONTENT).expect("write source");
    let hash = BlobHash::digest(CONTENT);
    let result = store
        .install_file(source, hash, CONTENT.len() as u64)
        .expect("install source file");
    assert_eq!(result.outcome, InstallOutcome::Installed);
}

#[test]
fn concurrent_identical_installs_publish_exactly_once() {
    let (_directory, store) = store();
    let store = Arc::new(store);
    let barrier = Arc::new(Barrier::new(8));
    let hash = BlobHash::digest(CONTENT);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                store
                    .install_reader(Cursor::new(CONTENT), hash, 1_024)
                    .expect("concurrent install")
                    .outcome
            })
        })
        .collect();
    let outcomes: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().expect("join installer"))
        .collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == InstallOutcome::Installed)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == InstallOutcome::AlreadyPresent)
            .count(),
        7
    );
    assert_eq!(
        store.verify(hash).expect("verify published blob").size,
        CONTENT.len() as u64
    );
    assert_eq!(incoming_files(&store, hash), 0);
}

#[test]
fn rejects_a_corrupt_existing_target() {
    let (_directory, store) = store();
    let hash = BlobHash::digest(CONTENT);
    store
        .install_reader(Cursor::new(CONTENT), hash, 1_024)
        .expect("initial install");
    fs::write(store.path_for(hash), vec![b'x'; CONTENT.len()]).expect("corrupt target");

    assert!(matches!(
        store.install_reader(Cursor::new(CONTENT), hash, 1_024),
        Err(BlobStoreError::CorruptBlob { .. })
    ));
    assert_eq!(incoming_files(&store, hash), 0);
}

#[test]
fn oversize_and_wrong_hash_leave_no_temporary_files() {
    let (_directory, store) = store();
    let expected = BlobHash::digest(CONTENT);
    assert!(matches!(
        store.install_reader(Cursor::new(CONTENT), expected, 4),
        Err(BlobStoreError::TooLarge { limit: 4 })
    ));
    assert_eq!(incoming_files(&store, expected), 0);

    let wrong = BlobHash::digest(b"different");
    assert!(matches!(
        store.install_reader(Cursor::new(CONTENT), wrong, 1_024),
        Err(BlobStoreError::HashMismatch { .. })
    ));
    assert_eq!(incoming_files(&store, wrong), 0);
}

#[test]
fn verified_removal_is_idempotent_and_refuses_corrupt_bytes() {
    let (_directory, store) = store();
    let hash = BlobHash::digest(CONTENT);
    store
        .install_reader(Cursor::new(CONTENT), hash, 1_024)
        .expect("install orphan candidate");

    assert!(
        store
            .remove_verified(hash, 1_024)
            .expect("remove verified orphan")
    );
    assert!(
        !store
            .remove_verified(hash, 1_024)
            .expect("missing orphan is an idempotent no-op")
    );

    store
        .install_reader(Cursor::new(CONTENT), hash, 1_024)
        .expect("reinstall candidate");
    fs::write(store.path_for(hash), b"corrupt").expect("corrupt stored bytes");
    assert!(store.remove_verified(hash, 1_024).is_err());
    assert!(
        store.path_for(hash).exists(),
        "cleanup must not unlink unverified contents"
    );
}

#[cfg(unix)]
#[test]
fn rejects_a_symlink_target() {
    use std::os::unix::fs::symlink;

    let (directory, store) = store();
    let hash = BlobHash::digest(CONTENT);
    let target = store.path_for(hash);
    fs::create_dir_all(target.parent().expect("shard")).expect("create shard");
    let destination = directory.path().join("elsewhere");
    fs::write(&destination, CONTENT).expect("write symlink destination");
    symlink(destination, &target).expect("create symlink");

    assert!(matches!(
        store.install_reader(Cursor::new(CONTENT), hash, 1_024),
        Err(BlobStoreError::UnsafeFilesystemEntry { .. })
    ));
    assert_eq!(incoming_files(&store, hash), 0);
}

#[cfg(unix)]
#[test]
fn rejects_a_symlink_storage_parent() {
    use std::os::unix::fs::symlink;

    let (directory, store) = store();
    fs::create_dir_all(store.root()).expect("create store root");
    let elsewhere = directory.path().join("elsewhere");
    fs::create_dir(&elsewhere).expect("create destination");
    symlink(&elsewhere, store.root().join("blobs")).expect("create symlink parent");
    let hash = BlobHash::digest(CONTENT);

    assert!(matches!(
        store.install_reader(Cursor::new(CONTENT), hash, 1_024),
        Err(BlobStoreError::UnsafeFilesystemEntry { .. })
    ));
    assert!(
        fs::read_dir(elsewhere)
            .expect("read destination")
            .next()
            .is_none()
    );
}
