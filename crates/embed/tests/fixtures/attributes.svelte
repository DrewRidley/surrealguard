<script lang="ts">
  export let minAge = 18;
  const rows = db.query("SELECT * OMIT scriptghost FROM user");
</script>

<h1>Dashboard — the “live” panel</h1>

<section class="panel">
  <p>Members over {minAge}</p>
  <Query title="Über" q="SELECT * OMIT ghostfield FROM user" />
</section>

<section class="feed">
  <LiveQuery title="Ähnlich" q="SELECT * FROM user WHERE phantomfield > {minAge}" />
</section>
