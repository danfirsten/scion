package demo;

public class Guard {
    public void run(Job job) {
        job.prepare();
        job.execute();
        job.finish();
    }
}
