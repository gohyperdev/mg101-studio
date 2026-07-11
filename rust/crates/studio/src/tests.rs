//! Testy wykonawcy na realnym profilu/katalogu MG-101 (przez dev-dep pack).

use super::*;
use mg101_commands::TargetRef;
use mg101_library::{LibraryPatch, MemoryStore, PatchOrigin};
use std::collections::BTreeSet;

fn fixture() -> (DeviceProfile, EffectCatalog) {
    mg101_pack_nux_mg101::load().expect("profil MG-101")
}

fn patch(id: &str, record_size: usize) -> LibraryPatch {
    LibraryPatch {
        id: id.into(),
        name: id.into(),
        blob: vec![0u8; record_size],
        origin: PatchOrigin::Created,
        device_id: "nux-mg101".into(),
        firmware: None,
        codec_version: "1".into(),
        content_hash: "c".into(),
        exact_hash: "e".into(),
        tags: BTreeSet::new(),
        groups: BTreeSet::new(),
        created_at: 0,
        updated_at: 0,
        revision: 1,
    }
}

fn lib_target(id: &str, rev: i64) -> TargetRef {
    TargetRef::Library {
        patch_id: id.into(),
        expected_revision: rev,
    }
}

fn studio_with_one() -> (Studio<'static, MemoryStore>, DeviceProfile, EffectCatalog) {
    // Wyciek profilu/katalogu do 'static dla ergonomii testów (jednorazowy).
    let (profile, catalog) = fixture();
    let profile: &'static DeviceProfile = Box::leak(Box::new(profile));
    let catalog: &'static EffectCatalog = Box::leak(Box::new(catalog));
    let mut store = MemoryStore::new();
    store.add(patch("p1", profile.record_size)).unwrap();
    (
        Studio::new(store, profile, catalog, 100),
        profile.clone(),
        catalog.clone(),
    )
}

#[test]
fn get_profile_reports_record_size_and_blocks() {
    let (mut s, _p, _c) = studio_with_one();
    let v = s.execute(&Command::GetProfile).unwrap();
    assert_eq!(v["recordSize"], 8402);
    assert!(v["blocks"].as_array().unwrap().len() >= 10);
    assert!(v["namedFields"].as_object().unwrap().contains_key("send"));
}

#[test]
fn set_name_bumps_revision_and_reflects_in_get_patch() {
    let (mut s, _p, _c) = studio_with_one();
    let r = s
        .execute(&Command::SetName {
            target: lib_target("p1", 1),
            name: "Lead".into(),
        })
        .unwrap();
    assert_eq!(r["revision"], 2);
    let v = s
        .execute(&Command::GetPatch {
            patch_id: "p1".into(),
        })
        .unwrap();
    assert_eq!(v["name"], "Lead");
    assert_eq!(v["revision"], 2);
}

#[test]
fn wrong_revision_is_conflict() {
    let (mut s, _p, _c) = studio_with_one();
    let err = s
        .execute(&Command::SetBpm {
            target: lib_target("p1", 99),
            bpm: 120,
        })
        .unwrap_err();
    assert!(matches!(
        err,
        ExecError::Conflict {
            current_revision: 1
        }
    ));
}

#[test]
fn set_model_then_parameter_roundtrips_through_get_patch() {
    let (mut s, profile, catalog) = studio_with_one();
    let block = &profile.blocks[2]; // efx — ma modele z parametrami
    let model = catalog.models(&block.id)[0].clone();
    let values: Vec<i64> = model.parameters.iter().map(|p| p.minimum()).collect();

    let r = s
        .execute(&Command::SetModel {
            target: lib_target("p1", 1),
            block: block.id.clone(),
            model: model.model_id,
            values,
            bypassed: false,
        })
        .unwrap();
    assert_eq!(r["success"], true);

    // get_patch pokazuje wybrany model dla tego bloku.
    let v = s
        .execute(&Command::GetPatch {
            patch_id: "p1".into(),
        })
        .unwrap();
    let blk = v["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["block"] == block.id)
        .unwrap();
    assert_eq!(blk["current_model"], model.model_id);
    assert_eq!(blk["bypassed"], false);

    // Zmiana pierwszego parametru, jeśli istnieje.
    if let Some(param) = model.parameters.first() {
        let target_val = param.maximum();
        s.execute(&Command::SetParameter {
            target: lib_target("p1", 2),
            block: block.id.clone(),
            parameter: param.name.clone(),
            value: target_val,
        })
        .unwrap();
        let v = s
            .execute(&Command::GetPatch {
                patch_id: "p1".into(),
            })
            .unwrap();
        let blk = v["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["block"] == block.id)
            .unwrap();
        let p0 = &blk["parameters"][0];
        assert_eq!(p0["value"], target_val);
    }
}

#[test]
fn set_bypass_and_named_field() {
    let (mut s, profile, _c) = studio_with_one();
    let block = &profile.blocks[0];
    s.execute(&Command::SetBypass {
        target: lib_target("p1", 1),
        block: block.id.clone(),
        bypassed: true,
    })
    .unwrap();
    let v = s
        .execute(&Command::GetPatch {
            patch_id: "p1".into(),
        })
        .unwrap();
    let blk = v["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["block"] == block.id)
        .unwrap();
    assert_eq!(blk["bypassed"], true);

    s.execute(&Command::SetNamedField {
        target: lib_target("p1", 2),
        field: "send".into(),
        value: 10,
    })
    .unwrap();
    let v = s
        .execute(&Command::GetPatch {
            patch_id: "p1".into(),
        })
        .unwrap();
    assert_eq!(v["named_fields"]["send"]["value"], 10);
}

#[test]
fn duplicate_creates_new_independent_patch() {
    let (mut s, _p, _c) = studio_with_one();
    let r = s
        .execute(&Command::DuplicatePatch {
            target: lib_target("p1", 1),
        })
        .unwrap();
    let new_id = r["newPatchID"].as_str().unwrap().to_string();
    assert_ne!(new_id, "p1");
    // Nowy patch istnieje, rewizja 1, origin Created.
    let dup = s.store().get(&new_id).unwrap();
    assert_eq!(dup.revision, 1);
    assert_eq!(dup.origin, PatchOrigin::Created);
    assert_eq!(
        s.execute(&Command::ListPatches)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn select_nonexistent_is_not_found() {
    let (mut s, _p, _c) = studio_with_one();
    let err = s
        .execute(&Command::SelectPatch {
            patch_id: "ghost".into(),
        })
        .unwrap_err();
    assert!(matches!(err, ExecError::NotFound(_)));
    // Wybór istniejącego widoczny w get_selection.
    s.execute(&Command::SelectPatch {
        patch_id: "p1".into(),
    })
    .unwrap();
    let sel = s.execute(&Command::GetSelection).unwrap();
    assert_eq!(sel["selectedPatchID"], "p1");
}

#[test]
fn list_models_returns_catalog_for_block() {
    let (mut s, profile, _c) = studio_with_one();
    let v = s
        .execute(&Command::ListModels {
            block: profile.blocks[2].id.clone(),
        })
        .unwrap();
    assert!(!v.as_array().unwrap().is_empty());
    assert!(v[0].get("modelID").is_some());
}

#[test]
fn file_target_rejected() {
    let (mut s, _p, _c) = studio_with_one();
    let err = s
        .execute(&Command::SetBpm {
            target: TargetRef::File {
                input: "a".into(),
                output: "b".into(),
            },
            bpm: 120,
        })
        .unwrap_err();
    assert!(matches!(err, ExecError::Unsupported(_)));
}
