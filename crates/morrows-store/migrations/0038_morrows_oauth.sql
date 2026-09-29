-- Control-plane-owned OAuth state, committed atomically for code use and rotation.
CREATE TABLE morrows_oauth_state (id INTEGER PRIMARY KEY CHECK(id=1), state TEXT NOT NULL);
INSERT INTO morrows_oauth_state VALUES (1, '{}');
