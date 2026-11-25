//! Retained `vq` contributors and the document's actual contributor list.
//! Wire serialization reads these same owners; document/history routing and
//! the real application profile, clock and notification services are separate.
use crate::{ContributorRecord, ContributorTicks};
use std::sync::{Arc, Mutex, MutexGuard};

fn guard<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

/// Retained source `apr` user profile. Its nullable raw strings have empty
/// getters, just as the source; two contributor roots may share this owner.
#[derive(Clone, Debug)]
pub struct ContributorProfile(Arc<Mutex<ProfileFields>>);
#[derive(Debug)]
struct ProfileFields {
    name: Option<Vec<u16>>,
    website: Option<Vec<u16>>,
    email: Option<Vec<u16>>,
}
impl ContributorProfile {
    pub fn new(name: Option<Vec<u16>>, website: Option<Vec<u16>>, email: Option<Vec<u16>>) -> Self {
        Self(Arc::new(Mutex::new(ProfileFields {
            name,
            website,
            email,
        })))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn name(&self) -> Vec<u16> {
        guard(&self.0).name.clone().unwrap_or_default()
    }
    pub fn website(&self) -> Vec<u16> {
        guard(&self.0).website.clone().unwrap_or_default()
    }
    pub fn email(&self) -> Vec<u16> {
        guard(&self.0).email.clone().unwrap_or_default()
    }
    pub fn set_name(&self, value: Option<Vec<u16>>) {
        guard(&self.0).name = value;
    }
    pub fn set_website(&self, value: Option<Vec<u16>>) {
        guard(&self.0).website = value;
    }
    pub fn set_email(&self, value: Option<Vec<u16>>) {
        guard(&self.0).email = value;
    }
}

/// One actual `vq` identity. Replacing its profile changes the same contributor
/// root and lets held references observe the source `eb.d` update path.
#[derive(Clone, Debug)]
pub struct Contributor(Arc<Mutex<ContributorFields>>);
#[derive(Debug)]
struct ContributorFields {
    profile: ContributorProfile,
    ticks: ContributorTicks,
    comment: Option<Vec<u16>>,
}
impl Contributor {
    pub fn new(
        profile: ContributorProfile,
        ticks: ContributorTicks,
        comment: Option<Vec<u16>>,
    ) -> Self {
        Self(Arc::new(Mutex::new(ContributorFields {
            profile,
            ticks,
            comment,
        })))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn profile(&self) -> ContributorProfile {
        guard(&self.0).profile.clone()
    }
    pub fn set_profile(&self, profile: ContributorProfile) {
        guard(&self.0).profile = profile;
    }
    pub fn ticks(&self) -> ContributorTicks {
        guard(&self.0).ticks
    }
    pub fn set_ticks(&self, ticks: ContributorTicks) {
        guard(&self.0).ticks = ticks;
    }
    pub fn comment(&self) -> Vec<u16> {
        guard(&self.0).comment.clone().unwrap_or_default()
    }
    pub fn set_comment(&self, comment: Option<Vec<u16>>) {
        guard(&self.0).comment = comment;
    }
    pub fn record(&self) -> ContributorRecord {
        let (profile, ticks, comment) = {
            let fields = guard(&self.0);
            (
                fields.profile.clone(),
                fields.ticks,
                fields.comment.clone().unwrap_or_default(),
            )
        };
        ContributorRecord {
            name: profile.name(),
            website: profile.website(),
            email: profile.email(),
            ticks,
            comment,
        }
    }
    pub(crate) fn from_record(record: ContributorRecord) -> Self {
        Self::new(
            ContributorProfile::new(Some(record.name), Some(record.website), Some(record.email)),
            record.ticks,
            Some(record.comment),
        )
    }
}

/// Required actual host effects at the corresponding `eb.d`/`eb.c` sites.
/// `names_equal` must implement the active source culture's ToLower comparison;
/// it is not an ASCII or locale-independent fallback. The caller must supply
/// its actual profile, DateTime.Now tick source, comment and document notifier.
pub trait ContributorEffects<E> {
    fn current_profile(&mut self) -> Result<ContributorProfile, E>;
    fn now(&mut self) -> Result<ContributorTicks, E>;
    fn current_comment(&mut self) -> Result<Option<Vec<u16>>, E>;
    fn names_equal(&mut self, left: &[u16], right: &[u16]) -> Result<bool, E>;
    fn notify_document(&mut self) -> Result<(), E>;
}

/// Effect failures and the source list-index failure after reentrant removal.
#[derive(Debug, PartialEq)]
pub enum ContributorError<E> {
    Effect(E),
    Index,
    CollectionModified,
}
impl<E> From<E> for ContributorError<E> {
    fn from(value: E) -> Self {
        Self::Effect(value)
    }
}

/// The actual contributor-list owner held by a GTX header. Cloning this handle
/// retains the same list, and saving the header reads its current contributors.
#[derive(Clone, Debug, Default)]
pub struct DocumentContributors(Arc<Mutex<ContributorList>>);
#[derive(Debug, Default)]
struct ContributorList {
    entries: Vec<Contributor>,
    revision: u64,
}
impl DocumentContributors {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn entries(&self) -> Vec<Contributor> {
        guard(&self.0).entries.clone()
    }
    pub fn records(&self) -> Vec<ContributorRecord> {
        self.entries().iter().map(Contributor::record).collect()
    }
    pub(crate) fn from_records(records: Vec<ContributorRecord>) -> Self {
        Self(Arc::new(Mutex::new(ContributorList {
            entries: records.into_iter().map(Contributor::from_record).collect(),
            revision: 0,
        })))
    }

    /// Source `eb.d`: update or prepend the current user, then insert distinct
    /// supplied contributor identities in source order. The returned identities
    /// are the actual inserted roots used by `eb.c`, not copies or indices.
    /// Host callback reentry/concurrent list mutation and CLR allocation timing
    /// remain uncharacterized; no document lock is held while invoking effects.
    pub fn capture<E>(
        &self,
        additional: Option<&[Contributor]>,
        effects: &mut impl ContributorEffects<E>,
    ) -> Result<Option<Vec<Contributor>>, ContributorError<E>> {
        let first = guard(&self.0).entries.first().cloned();
        let is_current = if let Some(first) = &first {
            let left = first.profile().name();
            let right = effects.current_profile()?.name();
            effects.names_equal(&left, &right)?
        } else {
            false
        };
        let mut inserted = Vec::new();
        if !is_current {
            let profile = effects.current_profile()?;
            let ticks = effects.now()?;
            let comment = effects.current_comment()?;
            let entry = Contributor::new(profile, ticks, comment);
            {
                let mut list = guard(&self.0);
                list.entries.insert(0, entry.clone());
                list.revision = list.revision.wrapping_add(1);
            }
            inserted.push(entry);
        } else {
            // eb.d reads h()[0] again after both name comparisons.
            let first = guard(&self.0)
                .entries
                .first()
                .cloned()
                .ok_or(ContributorError::Index)?;
            first.set_profile(effects.current_profile()?);
            first.set_ticks(effects.now()?);
            if first.comment().is_empty() {
                first.set_comment(effects.current_comment()?);
            }
            effects.notify_document()?;
        }
        if let Some(additional) = additional {
            let mut position = 1;
            for candidate in additional {
                let mut found = false;
                // The original foreach obtains an actual List enumerator. It
                // checks its captured version on each MoveNext, including the
                // terminal move; a successful early break does not move again.
                let revision = guard(&self.0).revision;
                let mut index = 0;
                loop {
                    let current = {
                        let list = guard(&self.0);
                        if list.revision != revision {
                            return Err(ContributorError::CollectionModified);
                        }
                        list.entries.get(index).cloned()
                    };
                    let Some(current) = current else {
                        break;
                    };
                    if effects
                        .names_equal(&candidate.profile().name(), &current.profile().name())?
                    {
                        found = true;
                        break;
                    }
                    index += 1;
                }
                if !found {
                    let mut list = guard(&self.0);
                    if position > list.entries.len() {
                        return Err(ContributorError::Index);
                    }
                    list.entries.insert(position, candidate.clone());
                    list.revision = list.revision.wrapping_add(1);
                    drop(list);
                    position += 1;
                    inserted.push(candidate.clone());
                }
            }
        }
        if inserted.is_empty() {
            Ok(None)
        } else {
            effects.notify_document()?;
            Ok(Some(inserted))
        }
    }

    /// Source `eb.c`: remove only matching contributor identities, then notify
    /// even if none were present. Surviving entries and held roots stay real.
    pub fn remove_captured<E>(
        &self,
        captured: &[Contributor],
        effects: &mut impl ContributorEffects<E>,
    ) -> Result<(), ContributorError<E>> {
        self.remove_captured_with(captured, || effects.notify_document())
    }

    // eb.c resolves its actual parent and notifies only after the removal loop.
    pub(crate) fn remove_captured_with<E>(
        &self,
        captured: &[Contributor],
        mut notify: impl FnMut() -> Result<(), E>,
    ) -> Result<(), ContributorError<E>> {
        for entry in captured {
            let removed = {
                let mut entries = guard(&self.0);
                let index = entries
                    .entries
                    .iter()
                    .position(|value| value.same_reference(entry));
                index.map(|index| {
                    entries.revision = entries.revision.wrapping_add(1);
                    entries.entries.remove(index)
                })
            };
            drop(removed);
        }
        notify()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BinaryWriter, Gtx375GraphDocument, PointwiseNode};
    fn text(value: &str) -> Vec<u16> {
        value.encode_utf16().collect()
    }
    struct Effects {
        profile: ContributorProfile,
        now: ContributorTicks,
        comment: Option<Vec<u16>>,
        notifications: usize,
        fail_notify: bool,
    }
    impl ContributorEffects<&'static str> for Effects {
        fn current_profile(&mut self) -> Result<ContributorProfile, &'static str> {
            Ok(self.profile.clone())
        }
        fn now(&mut self) -> Result<ContributorTicks, &'static str> {
            Ok(self.now)
        }
        fn current_comment(&mut self) -> Result<Option<Vec<u16>>, &'static str> {
            Ok(self.comment.clone())
        }
        fn names_equal(&mut self, a: &[u16], b: &[u16]) -> Result<bool, &'static str> {
            Ok(String::from_utf16_lossy(a).eq_ignore_ascii_case(&String::from_utf16_lossy(b)))
        }
        fn notify_document(&mut self) -> Result<(), &'static str> {
            self.notifications += 1;
            if self.fail_notify {
                Err("notification")
            } else {
                Ok(())
            }
        }
    }
    fn effects(name: &str) -> Effects {
        Effects {
            profile: ContributorProfile::new(Some(text(name)), Some(text("example")), None),
            now: ContributorTicks::new(123).unwrap(),
            comment: Some(text(" hello ")),
            notifications: 0,
            fail_notify: false,
        }
    }
    #[test]
    fn captured_real_list_and_profile_are_the_values_saved_by_the_document() {
        let mut document =
            Gtx375GraphDocument::<()>::new(vec![Some(PointwiseNode::solid_color().unwrap())])
                .unwrap();
        let owner = document.header().contributor_owner();
        let mut host = effects("Alice");
        let added = owner.capture(None, &mut host).unwrap().unwrap();
        assert_eq!(host.notifications, 1);
        assert!(owner.entries()[0].same_reference(&added[0]));
        assert!(added[0].profile().same_reference(&host.profile));
        host.profile.set_website(Some(text("updated.example")));
        let mut writer = BinaryWriter::default();
        document.save(&mut writer).unwrap();
        let restored = Gtx375GraphDocument::<()>::load(writer.bytes()).unwrap();
        assert_eq!(
            restored.header().contributors()[0].website,
            text("updated.example")
        );
        assert_eq!(restored.header().contributors()[0].comment, text("hello"));
        owner.remove_captured(&added, &mut host).unwrap();
        assert!(document.header().contributors().is_empty());
        assert_eq!(host.notifications, 2);
    }
    #[test]
    fn same_name_updates_existing_identity_and_duplicate_additions_preserve_real_roots() {
        let owner = DocumentContributors::new();
        let mut host = effects("Alice");
        let first = owner.capture(None, &mut host).unwrap().unwrap().remove(0);
        first.set_comment(None);
        let replacement = ContributorProfile::new(Some(text("ALICE")), None, Some(text("new")));
        host.profile = replacement.clone();
        host.now = ContributorTicks::new(456).unwrap();
        let extra = Contributor::new(
            ContributorProfile::new(Some(text("Bob")), None, None),
            ContributorTicks::new(55).unwrap(),
            None,
        );
        let duplicate = Contributor::new(
            ContributorProfile::new(Some(text("bob")), None, None),
            ContributorTicks::new(56).unwrap(),
            None,
        );
        let inserted = owner
            .capture(Some(&[extra.clone(), duplicate]), &mut host)
            .unwrap()
            .unwrap();
        assert_eq!(inserted.len(), 1);
        assert!(inserted[0].same_reference(&extra));
        assert!(owner.entries()[0].same_reference(&first));
        assert!(first.profile().same_reference(&replacement));
        assert_eq!(first.ticks().get(), 456);
        assert_eq!(first.comment(), text(" hello "));
        assert_eq!(host.notifications, 3); // initial insert, update, extra insert
        owner.remove_captured(&inserted, &mut host).unwrap();
        assert!(owner.entries()[0].same_reference(&first));
        assert_eq!(owner.entries().len(), 1);
    }
    #[test]
    fn notification_failure_keeps_committed_insert_without_fabricating_return() {
        let owner = DocumentContributors::new();
        let mut host = effects("Alice");
        host.fail_notify = true;
        assert!(matches!(
            owner.capture(None, &mut host),
            Err(ContributorError::Effect("notification"))
        ));
        assert_eq!(owner.entries().len(), 1);
        assert!(owner.entries()[0].profile().same_reference(&host.profile));
    }
    struct Reenter {
        inner: Effects,
        owner: DocumentContributors,
        victim: Contributor,
        compare_count: usize,
        trigger: usize,
        answer: bool,
    }
    impl ContributorEffects<&'static str> for Reenter {
        fn current_profile(&mut self) -> Result<ContributorProfile, &'static str> {
            self.inner.current_profile()
        }
        fn now(&mut self) -> Result<ContributorTicks, &'static str> {
            self.inner.now()
        }
        fn current_comment(&mut self) -> Result<Option<Vec<u16>>, &'static str> {
            self.inner.current_comment()
        }
        fn notify_document(&mut self) -> Result<(), &'static str> {
            self.inner.notify_document()
        }
        fn names_equal(&mut self, a: &[u16], b: &[u16]) -> Result<bool, &'static str> {
            self.compare_count += 1;
            if self.compare_count == self.trigger {
                self.owner
                    .remove_captured(std::slice::from_ref(&self.victim), &mut self.inner)
                    .unwrap();
                Ok(self.answer)
            } else {
                self.inner.names_equal(a, b)
            }
        }
    }
    #[test]
    fn matching_name_rereads_the_actual_first_after_host_reentry() {
        let owner = DocumentContributors::new();
        let mut host = effects("Alice");
        let first = owner.capture(None, &mut host).unwrap().unwrap().remove(0);
        let second = Contributor::new(
            ContributorProfile::new(Some(text("Bob")), None, None),
            ContributorTicks::new(3).unwrap(),
            None,
        );
        owner
            .capture(Some(std::slice::from_ref(&second)), &mut host)
            .unwrap();
        let profile = host.profile.clone();
        host.now = ContributorTicks::new(456).unwrap();
        let mut reentry = Reenter {
            inner: host,
            owner: owner.clone(),
            victim: first.clone(),
            compare_count: 0,
            trigger: 1,
            answer: true,
        };
        owner.capture(None, &mut reentry).unwrap();
        assert!(owner.entries()[0].same_reference(&second));
        assert!(second.profile().same_reference(&profile));
        assert_eq!(second.ticks().get(), 456);
        assert_eq!(first.ticks().get(), 123);
    }
    #[test]
    fn additional_name_enumeration_reports_source_version_failure_after_reentry() {
        let owner = DocumentContributors::new();
        let mut host = effects("Alice");
        let first = owner.capture(None, &mut host).unwrap().unwrap().remove(0);
        let extra = Contributor::new(
            ContributorProfile::new(Some(text("Bob")), None, None),
            ContributorTicks::new(3).unwrap(),
            None,
        );
        let mut reentry = Reenter {
            inner: host,
            owner: owner.clone(),
            victim: first,
            compare_count: 0,
            trigger: 2,
            answer: false,
        };
        assert!(matches!(
            owner.capture(Some(&[extra]), &mut reentry),
            Err(ContributorError::CollectionModified)
        ));
        assert!(owner.entries().is_empty());
    }
}
