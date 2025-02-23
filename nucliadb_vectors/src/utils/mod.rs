// Copyright (C) 2021 Bosutech XXI S.L.
//
// nucliadb is offered under the AGPL v3.0 and as commercial software.
// For commercial licensing, contact us at info@nuclia.com.
//
// AGPL:
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <http://www.gnu.org/licenses/>.
//

pub mod dtrie;
pub mod merger;
pub mod trie;

pub trait DeleteLog: std::marker::Sync {
    fn is_deleted(&self, _: &str) -> bool;
}
impl<'a, D: DeleteLog> DeleteLog for &'a D {
    fn is_deleted(&self, x: &str) -> bool {
        D::is_deleted(self, x)
    }
}
impl<Dl: DeleteLog, S: crate::disk::key_value::Slot> crate::disk::key_value::Slot for (Dl, S) {
    fn get_key<'a>(&self, x: &'a [u8]) -> &'a [u8] {
        self.1.get_key(x)
    }
    fn cmp_keys(&self, x: &[u8], key: &[u8]) -> std::cmp::Ordering {
        self.1.cmp_keys(x, key)
    }
    fn read_exact<'a>(&self, x: &'a [u8]) -> (/* head */ &'a [u8], /* tail */ &'a [u8]) {
        self.1.read_exact(x)
    }
    fn keep_in_merge(&self, x: &[u8]) -> bool {
        let key = std::str::from_utf8(self.get_key(x)).unwrap();
        !self.0.is_deleted(key)
    }
}
