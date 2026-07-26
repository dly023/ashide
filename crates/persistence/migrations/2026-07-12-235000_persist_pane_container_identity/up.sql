-- 每个 leaf pane 都有独立、跨重启稳定的容器身份。布局坐标和数据库自增 id
-- 都只是 locator，不能承担 Session Navigator 的用户状态身份。
CREATE TABLE pane_container_identities (
    pane_node_id INTEGER PRIMARY KEY NOT NULL,
    uuid BLOB NOT NULL,
    FOREIGN KEY (pane_node_id) REFERENCES pane_nodes(id) ON DELETE CASCADE
);

-- 旧 leaf 没有容器身份；为每个现存 pane 一次性分配全新 UUID。该值不从 tab/leaf
-- 坐标、数据库自增 id 或 terminal runtime UUID 推导，迁移后即成为唯一稳定身份。
INSERT INTO pane_container_identities (pane_node_id, uuid)
SELECT pane_node_id, randomblob(16) FROM pane_leaves;
