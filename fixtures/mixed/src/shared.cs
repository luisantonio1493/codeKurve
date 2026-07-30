namespace Mixed;
public class Invoice { public int Total() => 1; }
public class Maker { public Invoice Make() => new Invoice(); }
